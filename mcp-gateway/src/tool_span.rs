//! OTel spans for `tools/call`: one `execute_tool` span per call and one
//! `mcp.backend` child per outbound attempt, in the GenAI semconv shape the
//! trace UI already renders.

use serde_json::Value;
use tracing::Span;
use uuid::Uuid;

use crate::types::{MCPServerConfig, ServerType, codes};

/// How a `tools/call` ended, derived from the JSON-RPC response alone.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub label: &'static str,
    pub result_bytes: usize,
}

impl Outcome {
    pub fn is_ok(&self) -> bool {
        self.label == "ok"
    }
}

pub(crate) fn outcome_of(response: &Value) -> Outcome {
    let result_bytes = response
        .get("result")
        .map_or(0, |result| result.to_string().len());
    let label = match response.pointer("/error/code").and_then(Value::as_i64) {
        Some(codes::TOOL_BLOCKED) => "blocked",
        Some(codes::TOOL_ASK) => "awaiting_approval",
        Some(codes::AUTH_REQUIRED) => "auth_required",
        Some(codes::INVALID_PARAMS) => "invalid_params",
        Some(_) => "backend_error",
        None if response.pointer("/result/isError") == Some(&Value::Bool(true)) => "tool_error",
        None => "ok",
    };
    Outcome {
        label,
        result_bytes,
    }
}

/// Open the call's span. Only a verified call joins the caller's trace; any
/// other becomes a fresh root, so a self-made or foreign `traceparent` can
/// never place a span inside someone else's trace.
pub(crate) fn tool_call_span(
    tool: &str,
    entry: &'static str,
    req_id: &Value,
    agent_id: Uuid,
    verified_traceparent: Option<&str>,
    meta: bool,
) -> Span {
    let span = tracing::info_span!(
        parent: None,
        "execute_tool",
        otel.name = %format!("execute_tool {tool}"),
        otel.kind = "server",
        otel.status_code = tracing::field::Empty,
        gen_ai.operation.name = "execute_tool",
        gen_ai.tool.name = %tool,
        tool.name = %tool,
        gen_ai.tool.call.id = %req_id,
        mcp.method.name = "tools/call",
        agent.id = %agent_id,
        nasiko.mcp.entry = entry,
        nasiko.mcp.server_kind = if meta { "meta" } else { "" },
        nasiko.mcp.server = tracing::field::Empty,
        nasiko.mcp.connector_id = tracing::field::Empty,
        nasiko.mcp.outcome = tracing::field::Empty,
        nasiko.mcp.result_bytes = tracing::field::Empty,
        tool.arguments = tracing::field::Empty,
        tool.result = tracing::field::Empty,
        error.type = tracing::field::Empty,
    );
    if let Some(traceparent) = verified_traceparent {
        nasiko_observability::join_remote_trace(&span, traceparent);
    }
    span
}

/// Largest payload (arguments or result) a span carries; the rest is cut.
const MAX_CONTENT_BYTES: usize = 2048;

/// Attach the model-supplied arguments, truncated. Only ever called with the
/// arguments as the model sent them: connector credentials are injected later,
/// at the backend call, and never pass through here.
pub(crate) fn record_arguments(span: &Span, arguments: &Value) {
    span.record("tool.arguments", truncated(&arguments.to_string()).as_str());
}

/// Attach what the call returned: the text content when there is any, else the
/// error message or the raw result. Truncated like the arguments.
pub(crate) fn record_result(span: &Span, response: &Value) {
    span.record("tool.result", truncated(&result_text(response)).as_str());
}

fn result_text(response: &Value) -> String {
    if let Some(message) = response.pointer("/error/message").and_then(Value::as_str) {
        return message.to_string();
    }
    let result = response.get("result").unwrap_or(&Value::Null);
    let text: Vec<&str> = result
        .get("content")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if text.is_empty() {
        result.to_string()
    } else {
        text.join("\n")
    }
}

fn truncated(text: &str) -> String {
    let mut text = text.to_string();
    if text.len() > MAX_CONTENT_BYTES {
        let mut end = MAX_CONTENT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

/// Close the call's span with how it ended.
pub(crate) fn record_outcome(span: &Span, response: &Value) {
    let outcome = outcome_of(response);
    span.record("nasiko.mcp.outcome", outcome.label);
    span.record("nasiko.mcp.result_bytes", outcome.result_bytes as u64);
    if !outcome.is_ok() {
        span.record("error.type", outcome.label);
        span.record("otel.status_code", "ERROR");
    }
}

/// Note which backend a routed call resolved to.
pub(crate) fn record_server(span: &Span, server: &MCPServerConfig) {
    span.record("nasiko.mcp.server", server.name.as_str());
    span.record("nasiko.mcp.connector_id", server.connector_id.to_string());
    span.record("nasiko.mcp.server_kind", server_kind(server));
}

fn server_kind(server: &MCPServerConfig) -> &'static str {
    match (&server.kind, server.system, server.trusted) {
        (ServerType::Composio, _, _) => "composio",
        (_, true, _) => "system",
        (_, _, true) => "uploaded",
        _ => "generic",
    }
}

/// One outbound attempt. `attempt` is 2 for the retry after an endpoint refresh.
pub(crate) fn backend_span(server: &MCPServerConfig, forwarded_as: &str, attempt: u32) -> Span {
    let host = reqwest::Url::parse(&server.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    tracing::info_span!(
        "mcp.backend",
        otel.name = %format!("mcp.backend {}", server.name),
        otel.kind = "client",
        otel.status_code = tracing::field::Empty,
        nasiko.mcp.forwarded_as = %forwarded_as,
        attempt,
        server.address = %host,
        error.type = tracing::field::Empty,
    )
}

/// The traceparent to forward to a backend: the attempt's own span, and only
/// when the call arrived on the verified path.
pub(crate) fn outbound_traceparent(verified: bool, backend: &Span) -> Option<String> {
    verified
        .then(|| nasiko_observability::traceparent_of(backend))
        .flatten()
}

pub(crate) fn record_backend_failure(backend: &Span) {
    backend.record("error.type", "connection");
    backend.record("otel.status_code", "ERROR");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn error_with(code: i64) -> Value {
        json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": code, "message": "x" } })
    }

    #[test]
    fn every_error_code_maps_to_its_outcome() {
        let cases = [
            (codes::TOOL_BLOCKED, "blocked"),
            (codes::TOOL_ASK, "awaiting_approval"),
            (codes::AUTH_REQUIRED, "auth_required"),
            (codes::INVALID_PARAMS, "invalid_params"),
            (codes::INTERNAL_ERROR, "backend_error"),
            (-31999, "backend_error"),
        ];
        for (code, label) in cases {
            assert_eq!(outcome_of(&error_with(code)).label, label, "code {code}");
        }
    }

    #[test]
    fn a_tool_level_failure_is_distinct_from_a_protocol_error() {
        let response = json!({ "result": { "isError": true, "content": [] } });
        assert_eq!(outcome_of(&response).label, "tool_error");
    }

    #[test]
    fn a_plain_result_is_ok_and_sized() {
        let response = json!({ "result": { "content": [{ "type": "text", "text": "hi" }] } });
        let outcome = outcome_of(&response);
        assert!(outcome.is_ok());
        assert_eq!(outcome.result_bytes, response["result"].to_string().len());
    }

    #[test]
    fn long_arguments_are_cut_on_a_character_boundary() {
        let cut = truncated(&"é".repeat(MAX_CONTENT_BYTES));
        assert!(cut.ends_with('…'));
        assert!(cut.len() <= MAX_CONTENT_BYTES + '…'.len_utf8());
    }

    #[test]
    fn short_arguments_pass_through_whole() {
        assert_eq!(truncated(r#"{"a":1}"#), r#"{"a":1}"#);
    }

    #[test]
    fn a_result_is_its_text_content() {
        let response = json!({ "result": { "content": [
            { "type": "text", "text": "first" }, { "type": "text", "text": "second" }
        ] } });
        assert_eq!(result_text(&response), "first\nsecond");
    }

    #[test]
    fn an_error_result_is_its_message() {
        assert_eq!(result_text(&error_with(codes::TOOL_BLOCKED)), "x");
    }

    #[test]
    fn a_result_without_text_falls_back_to_its_json() {
        let response = json!({ "result": { "ok": true } });
        assert_eq!(result_text(&response), r#"{"ok":true}"#);
    }

    #[test]
    fn an_error_response_has_no_result_bytes() {
        assert_eq!(outcome_of(&error_with(codes::TOOL_BLOCKED)).result_bytes, 0);
    }
}
