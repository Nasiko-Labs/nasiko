use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use nasiko_types::{
    CapturePolicy, CodingAgentEventV1, CodingAgentTimestampQuality, CodingAgentToolAssociation,
    CodingAgentToolCallStatus,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

const BATCH_SIZE: i64 = 8;
const EXPORT_CONCURRENCY: usize = 8;
const STALE_AFTER: &str = "5 minutes";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const MAX_CONTENT_CHARS: usize = 2_000;
const MAX_ERROR_BYTES: usize = 4_096;

#[derive(sqlx::FromRow)]
struct ClaimedEvent {
    user_id: Uuid,
    event_id: String,
    payload: Value,
    session_id: String,
    otlp_attempts: i32,
    otlp_claim_id: Uuid,
    otlp_trace_delivered_at: Option<DateTime<Utc>>,
    otlp_log_delivered_at: Option<DateTime<Utc>>,
}

pub async fn run(db: PgPool, http: reqwest::Client, endpoint: String) {
    loop {
        match export_once(&db, &http, &endpoint).await {
            Ok(0) => tokio::time::sleep(POLL_INTERVAL).await,
            Ok(_) => {}
            Err(error) => {
                tracing::error!(%error, "coding-agent OTLP outbox poll failed");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

/// Claims and exports one bounded batch. Claims are committed before any HTTP
/// call; the claim UUID prevents a stale worker from finalizing a reclaimed row.
pub async fn export_once(
    db: &PgPool,
    http: &reqwest::Client,
    endpoint: &str,
) -> anyhow::Result<usize> {
    let claimed = claim_batch(db).await?;
    let count = claimed.len();
    let results = futures::stream::iter(claimed.into_iter().map(|row| async move {
        let event: CodingAgentEventV1 = match serde_json::from_value(row.payload.clone()) {
            Ok(event) => event,
            Err(error) => {
                mark_failed(db, &row, format!("invalid persisted event: {error}")).await?;
                return Ok::<(), anyhow::Error>(());
            }
        };
        let mut event = event;
        event.session.id = row.session_id.clone();
        let delivery = async {
            if row.otlp_trace_delivered_at.is_none() {
                post_json(
                    http,
                    &format!("{}/v1/traces", endpoint.trim_end_matches('/')),
                    &trace_payload(&event),
                )
                .await?;
                if !mark_trace_delivered(db, &row).await? {
                    return Ok::<(), anyhow::Error>(());
                }
            }
            if row.otlp_log_delivered_at.is_none() {
                post_json(
                    http,
                    &format!("{}/v1/logs", endpoint.trim_end_matches('/')),
                    &log_payload(&event),
                )
                .await?;
                mark_log_delivered(db, &row).await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = delivery {
            mark_failed(db, &row, error.to_string()).await?;
        }
        Ok(())
    }))
    .buffer_unordered(EXPORT_CONCURRENCY)
    .collect::<Vec<_>>()
    .await;
    for result in results {
        result?;
    }
    Ok(count)
}

async fn claim_batch(db: &PgPool) -> Result<Vec<ClaimedEvent>, sqlx::Error> {
    let claim_id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    let claimed = sqlx::query_as(
        r#"WITH ready AS (
               SELECT user_id, event_id
               FROM coding_agent_telemetry_events
               WHERE (otlp_state IN ('pending', 'failed')
                      AND (otlp_next_attempt_at IS NULL OR otlp_next_attempt_at <= now()))
                  OR (otlp_state = 'processing'
                      AND otlp_last_attempt_at < now() - $1::interval)
               ORDER BY received_at
               FOR UPDATE SKIP LOCKED
               LIMIT $2
           )
           UPDATE coding_agent_telemetry_events AS events
           SET otlp_state = 'processing', otlp_attempts = events.otlp_attempts + 1,
               otlp_last_attempt_at = now(), otlp_next_attempt_at = NULL,
               otlp_last_error = NULL, otlp_last_error_at = NULL, otlp_claim_id = $3
           FROM ready
           WHERE events.user_id = ready.user_id AND events.event_id = ready.event_id
            RETURNING events.user_id, events.event_id, events.payload, events.session_id,
                      events.otlp_attempts, events.otlp_claim_id,
                      events.otlp_trace_delivered_at, events.otlp_log_delivered_at"#,
    )
    .bind(STALE_AFTER)
    .bind(BATCH_SIZE)
    .bind(claim_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(claimed)
}

async fn mark_trace_delivered(db: &PgPool, row: &ClaimedEvent) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        r#"UPDATE coding_agent_telemetry_events
           SET otlp_trace_delivered_at = COALESCE(otlp_trace_delivered_at, now()),
               otlp_state = CASE WHEN otlp_log_delivered_at IS NOT NULL THEN 'delivered' ELSE otlp_state END,
               otlp_delivered_at = CASE WHEN otlp_log_delivered_at IS NOT NULL THEN now() ELSE otlp_delivered_at END,
               otlp_claim_id = CASE WHEN otlp_log_delivered_at IS NOT NULL THEN NULL ELSE otlp_claim_id END
           WHERE user_id = $1 AND event_id = $2
              AND otlp_state = 'processing' AND otlp_claim_id = $3"#,
    )
    .bind(row.user_id)
    .bind(&row.event_id)
    .bind(row.otlp_claim_id)
    .execute(db)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

async fn mark_log_delivered(db: &PgPool, row: &ClaimedEvent) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query(
        r#"UPDATE coding_agent_telemetry_events
           SET otlp_log_delivered_at = COALESCE(otlp_log_delivered_at, now()),
               otlp_state = CASE WHEN otlp_trace_delivered_at IS NOT NULL THEN 'delivered' ELSE otlp_state END,
               otlp_delivered_at = CASE WHEN otlp_trace_delivered_at IS NOT NULL THEN now() ELSE otlp_delivered_at END,
               otlp_last_error = NULL, otlp_next_attempt_at = NULL, otlp_claim_id = NULL
           WHERE user_id = $1 AND event_id = $2
             AND otlp_state = 'processing' AND otlp_claim_id = $3"#,
    )
    .bind(row.user_id)
    .bind(&row.event_id)
    .bind(row.otlp_claim_id)
    .execute(db)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

async fn mark_failed(db: &PgPool, row: &ClaimedEvent, error: String) -> Result<(), sqlx::Error> {
    let shift = row.otlp_attempts.clamp(1, 6) as u32 - 1;
    let delay_seconds = 1_i64 << shift;
    sqlx::query(
        r#"UPDATE coding_agent_telemetry_events
           SET otlp_state = 'failed', otlp_last_error = $4, otlp_last_error_at = now(),
               otlp_next_attempt_at = now() + make_interval(secs => $5),
               otlp_claim_id = NULL
           WHERE user_id = $1 AND event_id = $2
             AND otlp_state = 'processing' AND otlp_claim_id = $3"#,
    )
    .bind(row.user_id)
    .bind(&row.event_id)
    .bind(row.otlp_claim_id)
    .bind(truncate_bytes(&error, MAX_ERROR_BYTES))
    .bind(delay_seconds as f64)
    .execute(db)
    .await?;
    Ok(())
}

async fn post_json(http: &reqwest::Client, url: &str, payload: &Value) -> anyhow::Result<()> {
    let response = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(payload)
        .send()
        .await?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let remaining = MAX_ERROR_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() == MAX_ERROR_BYTES {
            break;
        }
    }
    anyhow::bail!(
        "OTLP collector returned {status}: {}",
        String::from_utf8_lossy(&bytes)
    )
}

pub(crate) fn trace_payload(event: &CodingAgentEventV1) -> Value {
    let trace_id = trace_id_for_event(event);
    let root_id = scoped_id(
        "root-span",
        &event.source.agent_name,
        &event.session.id,
        &event.turn.id,
        16,
    );
    let mut root_attributes = common_attributes(event);
    root_attributes.push(string_attr("gen_ai.operation.name", "invoke_agent"));
    if event.capture_policy == CapturePolicy::Content {
        if let Some(prompt) = &event.turn.prompt {
            root_attributes.push(string_attr(
                "gen_ai.input.messages",
                &truncate_chars(prompt, MAX_CONTENT_CHARS),
            ));
        }
        if let Some(response) = &event.turn.response {
            root_attributes.push(string_attr(
                "gen_ai.output.messages",
                &truncate_chars(response, MAX_CONTENT_CHARS),
            ));
        }
    }
    let mut spans = vec![span(
        &trace_id,
        &root_id,
        "",
        "coding_agent.turn",
        1,
        (event.turn.started_at, event.turn.ended_at),
        root_attributes,
    )];
    spans.extend(event.turn.llm_calls.iter().map(|call| {
        let mut attributes = common_attributes(event);
        attributes.extend([
            string_attr("gen_ai.operation.name", "chat"),
            string_attr("gen_ai.system", &call.provider),
            string_attr("gen_ai.provider.name", &call.provider),
            string_attr("gen_ai.request.model", &call.model),
            string_attr("gen_ai.response.model", &call.model),
            int_attr("gen_ai.usage.input_tokens", call.input_tokens),
            int_attr("gen_ai.usage.output_tokens", call.output_tokens),
            int_attr(
                "gen_ai.usage.cache_read_input_tokens",
                call.cache_read_tokens,
            ),
            int_attr(
                "gen_ai.usage.cache_creation_input_tokens",
                call.cache_creation_tokens,
            ),
        ]);
        span(
            &trace_id,
            &scoped_id(
                "call-span",
                &event.source.agent_name,
                &event.session.id,
                &call.id,
                16,
            ),
            &root_id,
            &format!("chat {}", call.model),
            3,
            (call.started_at, call.ended_at),
            attributes,
        )
    }));
    spans.extend(event.turn.tool_calls.iter().map(|tool| {
        let mut attributes = common_attributes(event);
        attributes.extend([
            string_attr("gen_ai.operation.name", "execute_tool"),
            string_attr("tool.call.id", &tool.id),
            string_attr("tool.name", &tool.name),
            string_attr("tool.kind", &tool.kind),
            string_attr("tool.status", tool_status(tool.status)),
            string_attr("tool.association", tool_association(tool.association)),
            string_attr(
                "tool.timestamp_quality",
                timestamp_quality(tool.timestamp_quality),
            ),
            int_attr("tool.duration_ms", tool.duration_ms.unwrap_or(0)),
        ]);
        if let Some(model_call_id) = &tool.model_call_id {
            attributes.push(string_attr("gen_ai.model_call.id", model_call_id));
            if let Some(call) = event
                .turn
                .llm_calls
                .iter()
                .find(|call| &call.id == model_call_id)
            {
                attributes.extend([
                    string_attr("gen_ai.provider.name", &call.provider),
                    string_attr("gen_ai.request.model", &call.model),
                ]);
            }
        }
        if event.capture_policy == CapturePolicy::Content {
            if let Some(arguments) = &tool.arguments {
                attributes.push(string_attr("tool.arguments", &bounded_json(arguments)));
            }
            if let Some(output) = &tool.output {
                attributes.push(string_attr("tool.result", &bounded_json(output)));
            }
            if let Some(error) = &tool.error {
                attributes.push(string_attr(
                    "error.message",
                    &truncate_chars(error, MAX_CONTENT_CHARS),
                ));
            }
        }
        let started_at = tool.started_at.unwrap_or(event.turn.started_at);
        let ended_at = tool.ended_at.unwrap_or(started_at).max(started_at);
        let mut value = span(
            &trace_id,
            &scoped_id(
                "tool-span",
                &event.source.agent_name,
                &event.session.id,
                &tool.id,
                16,
            ),
            &root_id,
            &format!("execute_tool {}", tool.name),
            1,
            (started_at, ended_at),
            attributes,
        );
        value["status"] = match tool.status {
            CodingAgentToolCallStatus::Succeeded => json!({"code": 1}),
            CodingAgentToolCallStatus::Failed
            | CodingAgentToolCallStatus::Denied
            | CodingAgentToolCallStatus::TimedOut
            | CodingAgentToolCallStatus::Cancelled => json!({"code": 2}),
            _ => json!({}),
        };
        value
    }));
    json!({
        "resourceSpans": [{
            "resource": { "attributes": resource_attributes(event) },
            "scopeSpans": [{ "scope": { "name": "nasiko-server-coding-agent" }, "spans": spans }]
        }]
    })
}

pub(crate) fn log_payload(event: &CodingAgentEventV1) -> Value {
    let calls = &event.turn.llm_calls;
    let model = calls.last().map(|call| call.model.as_str()).unwrap_or("");
    let input = calls
        .iter()
        .fold(0_u64, |total, call| total.saturating_add(call.input_tokens));
    let output = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.output_tokens)
    });
    let cache_read = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.cache_read_tokens)
    });
    let cache_creation = calls.iter().fold(0_u64, |total, call| {
        total.saturating_add(call.cache_creation_tokens)
    });
    let mut attributes = common_attributes(event);
    attributes.extend([
        string_attr("coding_agent.turn.id", &event.turn.id),
        string_attr("coding_agent.source.id", &event.source.agent_id),
        string_attr(
            "coding_agent.capture_policy",
            match event.capture_policy {
                CapturePolicy::MetadataOnly => "metadata_only",
                CapturePolicy::Content => "content",
            },
        ),
        string_attr("gen_ai.request.model", model),
        int_attr("gen_ai.usage.input_tokens", input),
        int_attr("gen_ai.usage.output_tokens", output),
        int_attr("gen_ai.usage.cache_read_input_tokens", cache_read),
        int_attr("gen_ai.usage.cache_creation_input_tokens", cache_creation),
    ]);
    let root_span_id = scoped_id(
        "root-span",
        &event.source.agent_name,
        &event.session.id,
        &event.turn.id,
        16,
    );
    let mut records = vec![json!({
        "timeUnixNano": unix_nanos(event.turn.ended_at),
        "observedTimeUnixNano": unix_nanos(event.captured_at),
        "severityNumber": 9,
        "severityText": "INFO",
        "body": { "stringValue": "coding_agent.turn.completed" },
        "attributes": attributes,
        "traceId": trace_id_for_event(event),
        "spanId": root_span_id,
    })];
    records.extend(event.turn.tool_calls.iter().map(|tool| {
        let mut attributes = common_attributes(event);
        attributes.extend([
            string_attr("coding_agent.turn.id", &event.turn.id),
            string_attr("tool.call.id", &tool.id),
            string_attr("tool.name", &tool.name),
            string_attr("tool.kind", &tool.kind),
            string_attr("tool.status", tool_status(tool.status)),
            int_attr("tool.duration_ms", tool.duration_ms.unwrap_or(0)),
        ]);
        if let Some(model_call_id) = &tool.model_call_id {
            attributes.push(string_attr("gen_ai.model_call.id", model_call_id));
        }
        json!({
            "timeUnixNano": unix_nanos(tool.ended_at.or(tool.started_at).unwrap_or(event.turn.ended_at)),
            "observedTimeUnixNano": unix_nanos(event.captured_at),
            "severityNumber": 9,
            "severityText": "INFO",
            "body": { "stringValue": "coding_agent.tool.completed" },
            "attributes": attributes,
            "traceId": trace_id_for_event(event),
            "spanId": scoped_id("tool-span", &event.source.agent_name, &event.session.id, &tool.id, 16),
        })
    }));
    json!({
        "resourceLogs": [{
            "resource": { "attributes": resource_attributes(event) },
            "scopeLogs": [{
                "scope": { "name": "nasiko-server-coding-agent" },
                "logRecords": records
            }]
        }]
    })
}

fn tool_status(status: CodingAgentToolCallStatus) -> &'static str {
    match status {
        CodingAgentToolCallStatus::Pending => "pending",
        CodingAgentToolCallStatus::Running => "running",
        CodingAgentToolCallStatus::Succeeded => "succeeded",
        CodingAgentToolCallStatus::Failed => "failed",
        CodingAgentToolCallStatus::Denied => "denied",
        CodingAgentToolCallStatus::TimedOut => "timed_out",
        CodingAgentToolCallStatus::Cancelled => "cancelled",
        CodingAgentToolCallStatus::Unknown => "unknown",
    }
}

fn tool_association(association: CodingAgentToolAssociation) -> &'static str {
    match association {
        CodingAgentToolAssociation::Exact => "exact",
        CodingAgentToolAssociation::Turn => "turn",
        CodingAgentToolAssociation::Unknown => "unknown",
    }
}

fn timestamp_quality(quality: CodingAgentTimestampQuality) -> &'static str {
    match quality {
        CodingAgentTimestampQuality::Exact => "exact",
        CodingAgentTimestampQuality::Inferred => "inferred",
        CodingAgentTimestampQuality::Receipt => "receipt",
        CodingAgentTimestampQuality::Unknown => "unknown",
    }
}

fn bounded_json(value: &Value) -> String {
    truncate_chars(
        &serde_json::to_string(value).unwrap_or_else(|_| "null".into()),
        MAX_CONTENT_CHARS,
    )
}

pub(crate) fn trace_id_for_event(event: &CodingAgentEventV1) -> String {
    scoped_id(
        "trace",
        &event.source.agent_name,
        &event.session.id,
        &event.turn.id,
        32,
    )
}

fn common_attributes(event: &CodingAgentEventV1) -> Vec<Value> {
    vec![
        string_attr("session.id", &event.session.id),
        string_attr("agent.id", &event.source.agent_name),
        bool_attr("nasiko.synthetic", true),
        string_attr("nasiko.origin", "coding_agent"),
    ]
}

fn resource_attributes(event: &CodingAgentEventV1) -> Vec<Value> {
    vec![
        string_attr("service.name", &event.source.agent_name),
        string_attr("agent.id", &event.source.agent_name),
        string_attr("coding_agent.source.id", &event.source.agent_id),
    ]
}

fn span(
    trace_id: &str,
    span_id: &str,
    parent_span_id: &str,
    name: &str,
    kind: u8,
    times: (DateTime<Utc>, DateTime<Utc>),
    attributes: Vec<Value>,
) -> Value {
    let (started_at, ended_at) = times;
    json!({
        "traceId": trace_id, "spanId": span_id, "parentSpanId": parent_span_id,
        "name": name, "kind": kind,
        "startTimeUnixNano": unix_nanos(started_at),
        "endTimeUnixNano": unix_nanos(ended_at),
        "attributes": attributes, "status": {}
    })
}

fn string_attr(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn int_attr(key: &str, value: u64) -> Value {
    json!({ "key": key, "value": { "intValue": value.to_string() } })
}

fn bool_attr(key: &str, value: bool) -> Value {
    json!({ "key": key, "value": { "boolValue": value } })
}

fn unix_nanos(at: DateTime<Utc>) -> String {
    at.timestamp_nanos_opt().unwrap_or(0).to_string()
}

fn scoped_id(domain: &str, service: &str, session: &str, identity: &str, len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"nasiko-cli-integration-id-v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(b"\0");
    hasher.update(service.as_bytes());
    hasher.update(b"\0");
    hasher.update(session.as_bytes());
    hasher.update(b"\0");
    hasher.update(identity.as_bytes());
    hex::encode(hasher.finalize())[..len].to_owned()
}

fn truncate_chars(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_owned()
    } else {
        value
            .chars()
            .take(max.saturating_sub(3))
            .collect::<String>()
            + "..."
    }
}

fn truncate_bytes(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CodingAgentLlmCall, CodingAgentSession, CodingAgentSource,
        CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCall,
        CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
    };

    fn event(policy: CapturePolicy) -> CodingAgentEventV1 {
        let start = Utc.timestamp_opt(1_700_000_000, 123_000_000).unwrap();
        let end = Utc.timestamp_opt(1_700_000_002, 456_000_000).unwrap();
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id("claude", "session", "turn"),
            captured_at: end,
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "coding-agent".into(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id("claude", "session"),
                source_id: "session".into(),
            },
            turn: CodingAgentTurn {
                id: "turn".into(),
                prompt: (policy == CapturePolicy::Content).then(|| "prompt".into()),
                response: (policy == CapturePolicy::Content).then(|| "response".into()),
                started_at: start,
                ended_at: end,
                llm_calls: ["one", "two"]
                    .into_iter()
                    .map(|id| CodingAgentLlmCall {
                        id: id.into(),
                        provider: "anthropic".into(),
                        model: format!("model-{id}"),
                        input_tokens: 2,
                        output_tokens: 3,
                        cache_read_tokens: 5,
                        cache_creation_tokens: 7,
                        started_at: start,
                        ended_at: end,
                    })
                    .collect(),
                tool_calls: vec![],
            },
            capture_policy: policy,
        }
    }

    fn spans(payload: &Value) -> &Vec<Value> {
        payload["resourceSpans"][0]["scopeSpans"][0]["spans"]
            .as_array()
            .unwrap()
    }

    fn attr<'a>(attributes: &'a Value, key: &str) -> Option<&'a Value> {
        attributes
            .as_array()?
            .iter()
            .find(|value| value["key"] == key)
            .map(|value| &value["value"])
    }

    #[test]
    fn trace_shape_ids_timestamps_and_hierarchy_are_deterministic() {
        let event = event(CapturePolicy::Content);
        let payload = trace_payload(&event);
        let spans = spans(&payload);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["name"], "coding_agent.turn");
        assert_eq!(spans[1]["name"], "chat model-one");
        assert_eq!(spans[2]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[0]["traceId"], spans[2]["traceId"]);
        assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(spans[0]["spanId"].as_str().unwrap().len(), 16);
        assert_eq!(spans[0]["startTimeUnixNano"], "1700000000123000000");
        assert_eq!(trace_payload(&event), payload);
        assert_eq!(
            trace_id_for_event(&event),
            "7acc01114f6230dde2bf201f13490d3f"
        );
    }

    #[test]
    fn content_policy_controls_only_trace_content() {
        let content = trace_payload(&event(CapturePolicy::Content));
        assert_eq!(
            attr(&spans(&content)[0]["attributes"], "gen_ai.input.messages").unwrap()["stringValue"],
            "prompt"
        );
        let metadata = trace_payload(&event(CapturePolicy::MetadataOnly));
        assert!(attr(&spans(&metadata)[0]["attributes"], "gen_ai.input.messages").is_none());
    }

    #[test]
    fn linked_log_has_structured_usage_and_never_content() {
        let event = event(CapturePolicy::Content);
        let log = log_payload(&event);
        let record = &log["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
        assert_eq!(record["body"]["stringValue"], "coding_agent.turn.completed");
        assert_eq!(record["severityText"], "INFO");
        assert_eq!(record["traceId"], trace_id_for_event(&event));
        assert_eq!(record["spanId"], spans(&trace_payload(&event))[0]["spanId"]);
        assert_eq!(record["timeUnixNano"], "1700000002456000000");
        assert_eq!(
            attr(&record["attributes"], "gen_ai.usage.output_tokens").unwrap()["intValue"],
            "6"
        );
        let encoded = serde_json::to_string(&log).unwrap();
        assert!(!encoded.contains("prompt"));
        assert!(!encoded.contains("response"));
    }

    #[test]
    fn content_is_bounded_by_unicode_characters() {
        let mut event = event(CapturePolicy::Content);
        event.turn.prompt = Some("x".repeat(MAX_CONTENT_CHARS + 10));
        let payload = trace_payload(&event);
        let value = attr(&spans(&payload)[0]["attributes"], "gen_ai.input.messages").unwrap()["stringValue"].as_str().unwrap();
        assert_eq!(value.chars().count(), MAX_CONTENT_CHARS);
    }

    #[test]
    fn tool_spans_and_linked_logs_are_deterministic_and_policy_safe() {
        let mut event = event(CapturePolicy::Content);
        event.turn.tool_calls.push(CodingAgentToolCall {
            id: "native-1".into(),
            name: "shell".into(),
            kind: "tool".into(),
            model_call_id: Some("one".into()),
            status: CodingAgentToolCallStatus::Failed,
            arguments: Some(json!({"command": "secret-command"})),
            output: Some(json!("secret-result")),
            raw: None,
            error: Some("secret-error".into()),
            started_at: Some(event.turn.started_at),
            ended_at: Some(event.turn.ended_at),
            duration_ms: Some(2333),
            association: CodingAgentToolAssociation::Exact,
            timestamp_quality: CodingAgentTimestampQuality::Exact,
        });
        let trace = trace_payload(&event);
        let tool = &spans(&trace)[3];
        assert_eq!(tool["parentSpanId"], spans(&trace)[0]["spanId"]);
        assert_eq!(tool["kind"], 1);
        assert_eq!(tool["status"]["code"], 2);
        assert_eq!(
            attr(&tool["attributes"], "gen_ai.operation.name").unwrap()["stringValue"],
            "execute_tool"
        );
        assert_eq!(trace_payload(&event), trace);

        let logs = log_payload(&event);
        let records = logs["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
            .as_array()
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["spanId"], tool["spanId"]);
        let encoded = serde_json::to_string(&logs).unwrap();
        assert!(!encoded.contains("secret-command"));
        assert!(!encoded.contains("secret-result"));
        assert!(!encoded.contains("secret-error"));

        event.capture_policy = CapturePolicy::MetadataOnly;
        event.turn.prompt = None;
        event.turn.response = None;
        event.turn.tool_calls[0].arguments = None;
        event.turn.tool_calls[0].output = None;
        event.turn.tool_calls[0].error = None;
        assert!(
            !serde_json::to_string(&trace_payload(&event))
                .unwrap()
                .contains("error.message")
        );
    }
}
