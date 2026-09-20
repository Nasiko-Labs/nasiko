//! Thin wrapper around the official `a2a` crate (a2a-lf) with nasiko-specific helpers.

pub use a2a::{
    Artifact, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, Message, Part, PartContent,
    Role, SendMessageConfiguration, SendMessageRequest, StreamResponse, Task,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent, new_artifact_id,
    new_context_id, new_message_id, new_task_id,
};

// ─── Part constructors ──────────────────────────────────────────────────────

pub fn text_part(s: impl Into<String>) -> Part {
    Part {
        content: PartContent::Text(s.into()),
        filename: None,
        media_type: None,
        metadata: None,
    }
}

pub fn file_part(data: Vec<u8>, filename: Option<String>, media_type: Option<String>) -> Part {
    Part {
        content: PartContent::Raw(data),
        filename,
        media_type,
        metadata: None,
    }
}

pub fn data_part(value: serde_json::Value) -> Part {
    Part {
        content: PartContent::Data(value),
        filename: None,
        media_type: None,
        metadata: None,
    }
}

// ─── StreamResponse constructors ────────────────────────────────────────────

pub fn status_event(event: TaskStatusUpdateEvent) -> StreamResponse {
    StreamResponse::StatusUpdate(event)
}

pub fn artifact_event(event: TaskArtifactUpdateEvent) -> StreamResponse {
    StreamResponse::ArtifactUpdate(event)
}

pub fn task_event(task: Task) -> StreamResponse {
    StreamResponse::Task(task)
}

pub fn to_sse_data(event: &StreamResponse) -> String {
    serde_json::to_string(event).unwrap_or_default()
}

// ─── TaskStatusUpdateEvent constructors ─────────────────────────────────────

pub fn working(task_id: &str, context_id: &str) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Working,
            message: None,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

pub fn working_with_message(
    task_id: &str,
    context_id: &str,
    msg: Message,
) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Working,
            message: Some(msg),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

pub fn completed(task_id: &str, context_id: &str) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: None,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

pub fn failed(task_id: &str, context_id: &str, error_msg: &str) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Failed,
            message: Some(agent_message(context_id, task_id, text_part(error_msg))),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

// ─── TaskArtifactUpdateEvent constructors ───────────────────────────────────

pub fn text_chunk(
    task_id: &str,
    context_id: &str,
    artifact_id: &str,
    text: &str,
    append: bool,
    last_chunk: bool,
) -> TaskArtifactUpdateEvent {
    TaskArtifactUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        artifact: Artifact {
            artifact_id: artifact_id.into(),
            name: None,
            description: None,
            parts: vec![text_part(text)],
            metadata: None,
            extensions: None,
        },
        append: Some(append),
        last_chunk: Some(last_chunk),
        metadata: None,
    }
}

// ─── Message constructors ───────────────────────────────────────────────────

pub fn agent_message(context_id: &str, task_id: &str, part: Part) -> Message {
    Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(context_id.into()),
        task_id: Some(task_id.into()),
        role: Role::Agent,
        parts: vec![part],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    }
}

// ─── Request builders ───────────────────────────────────────────────────────

pub fn build_send_request(text: &str, context_id: Option<&str>) -> JsonRpcRequest {
    build_request("SendMessage", text, context_id, &[])
}

pub fn build_stream_request(text: &str, context_id: Option<&str>) -> JsonRpcRequest {
    build_request("SendStreamingMessage", text, context_id, &[])
}

pub fn build_stream_request_with_parts(
    text: &str,
    context_id: Option<&str>,
    extra_parts: &[Part],
) -> JsonRpcRequest {
    build_request("SendStreamingMessage", text, context_id, extra_parts)
}

pub fn build_stream_request_with_metadata(
    text: &str,
    context_id: Option<&str>,
    metadata: serde_json::Value,
) -> JsonRpcRequest {
    let mut req = build_request("SendStreamingMessage", text, context_id, &[]);
    if let Some(params) = req.params.as_mut()
        && let Some(obj) = params.as_object_mut()
    {
        obj.insert("metadata".to_string(), metadata);
    }
    req
}

// ─── Response extractors ────────────────────────────────────────────────────

/// Extract text content from an A2A JSONRPC result value.
/// Supports `artifacts[].parts[].text`, `status.message.parts[].text`,
/// and `message.parts[].text`.
pub fn extract_text(result: &serde_json::Value) -> Option<String> {
    // v1.0 wraps in "task", v0.3 is flat
    let task = result.get("task").unwrap_or(result);

    // Parts within one artifact/message are contiguous chunks (streaming
    // agents emit one part per token) — concatenate them directly. The same
    // applies to consecutive artifacts sharing an artifactId (a2a servers
    // accumulate each streamed chunk as its own artifact entry). Only
    // distinct artifacts get a newline between them.
    if let Some(artifacts) = task.get("artifacts").and_then(|a| a.as_array()) {
        let mut artifact_texts: Vec<String> = Vec::new();
        let mut last_id: Option<&str> = None;
        for artifact in artifacts {
            let id = artifact.get("artifactId").and_then(|v| v.as_str());
            if let Some(parts) = artifact.get("parts").and_then(|p| p.as_array()) {
                let text: String = parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(|v| v.as_str()))
                    .collect();
                if !text.is_empty() {
                    match (id, last_id, artifact_texts.last_mut()) {
                        (Some(id), Some(prev), Some(acc)) if id == prev => acc.push_str(&text),
                        _ => artifact_texts.push(text),
                    }
                }
            }
            last_id = id;
        }
        if !artifact_texts.is_empty() {
            return Some(artifact_texts.join("\n"));
        }
    }

    for parts_path in ["/status/message/parts", "/message/parts"] {
        let parts = if parts_path == "/status/message/parts" {
            task.pointer(parts_path)
        } else {
            result.pointer(parts_path)
        };
        if let Some(parts) = parts.and_then(|p| p.as_array()) {
            let text: String = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}

// ─── SSE stream event classification ────────────────────────────────────────

/// One semantic event decoded from an A2A SSE `data:` payload.
///
/// A single payload can carry several (e.g. a working-status message with
/// multiple parts), so [`classify_sse_event`] returns a list.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    /// A chunk of the agent's reply text (artifact update).
    ArtifactText(String),
    /// Working-status text — the agent's own progress narration
    /// (e.g. `"web_search: <query>"`).
    StatusText(String),
    /// Working-status structured data part (orchestrator events like
    /// `tool_call` / `thinking` are delivered this way).
    StatusData(serde_json::Value),
    /// Terminal: the task completed. `snapshot_text` carries the full text
    /// when the closing event was a task snapshot with artifacts (servers
    /// that answer a stream request with a single task object).
    Completed { snapshot_text: Option<String> },
    /// Terminal: the task failed or was canceled.
    Failed { reason: String },
}

/// Classify one A2A SSE `data:` JSON payload into semantic events.
///
/// Accepts every wire shape the platform produces: JSONRPC-wrapped
/// (`{"result": {...}}`) or bare; proto-style (`statusUpdate` /
/// `artifactUpdate` / `task` keys, `TASK_STATE_*` states) or legacy
/// kind-tagged (`"kind": "status-update"`, lowercase states).
pub fn classify_sse_event(event: &serde_json::Value) -> Vec<SseEvent> {
    let result = event.get("result").unwrap_or(event);
    let mut out = Vec::new();

    if let Some(update) = result.get("artifactUpdate") {
        collect_artifact_text(update, &mut out);
        return out;
    }
    if let Some(update) = result.get("statusUpdate") {
        classify_status(update, &mut out);
        return out;
    }
    if result.get("message").is_some() {
        // Bare message reply — agents without a task lifecycle (e.g. the
        // official a2a-go SDK) answer a stream request with a single
        // terminal message event.
        out.push(SseEvent::Completed {
            snapshot_text: extract_text(result),
        });
        return out;
    }
    if let Some(task) = result.get("task") {
        // Full task snapshot: terminal only when it says so — a bare
        // submission echo (state=submitted/working) is not an event.
        match task_state(task) {
            SseTaskState::Completed => {
                out.push(SseEvent::Completed {
                    snapshot_text: extract_text(result),
                });
            }
            SseTaskState::Failed => {
                out.push(SseEvent::Failed {
                    reason: failure_reason(task),
                });
            }
            SseTaskState::Working | SseTaskState::Other => {}
        }
        return out;
    }
    // Legacy kind-tagged shape.
    match result.get("kind").and_then(|k| k.as_str()) {
        Some("artifact-update") => collect_artifact_text(result, &mut out),
        Some("status-update") => classify_status(result, &mut out),
        _ => {}
    }
    out
}

fn classify_status(update: &serde_json::Value, out: &mut Vec<SseEvent>) {
    match task_state(update) {
        SseTaskState::Failed => {
            out.push(SseEvent::Failed {
                reason: failure_reason(update),
            });
        }
        SseTaskState::Completed => {
            out.push(SseEvent::Completed {
                snapshot_text: None,
            });
        }
        SseTaskState::Working | SseTaskState::Other => {
            if let Some(parts) = update
                .pointer("/status/message/parts")
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(data) = part.get("data") {
                        out.push(SseEvent::StatusData(data.clone()));
                    } else if let Some(text) = part.get("text").and_then(|t| t.as_str())
                        && !text.trim().is_empty()
                    {
                        out.push(SseEvent::StatusText(text.to_string()));
                    }
                }
            }
        }
    }
}

fn collect_artifact_text(update: &serde_json::Value, out: &mut Vec<SseEvent>) {
    // {"artifact": {"parts": [...]}} or {"parts": [...]} directly
    let parts = update
        .pointer("/artifact/parts")
        .or_else(|| update.get("parts"))
        .and_then(|p| p.as_array());
    if let Some(parts) = parts {
        let text: String = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect();
        if !text.is_empty() {
            out.push(SseEvent::ArtifactText(text));
        }
    }
}

/// Task lifecycle state as read off the wire, normalized across the
/// proto-style (`TASK_STATE_*`) and legacy lowercase spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SseTaskState {
    Working,
    Completed,
    /// Failed or canceled — both end the task without a usable answer.
    Failed,
    /// Submitted, unknown, or absent.
    Other,
}

fn task_state(v: &serde_json::Value) -> SseTaskState {
    match v
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("")
    {
        "TASK_STATE_WORKING" | "working" => SseTaskState::Working,
        "TASK_STATE_COMPLETED" | "completed" => SseTaskState::Completed,
        "TASK_STATE_FAILED" | "TASK_STATE_CANCELED" | "failed" | "canceled" => SseTaskState::Failed,
        _ => SseTaskState::Other,
    }
}

fn failure_reason(v: &serde_json::Value) -> String {
    v.pointer("/status/message/parts/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("task failed")
        .to_string()
}

pub fn extract_text_from_response(response: &JsonRpcResponse) -> Option<String> {
    extract_text(response.result.as_ref()?)
}

// ─── Private ────────────────────────────────────────────────────────────────

fn build_request(
    method: &str,
    text: &str,
    context_id: Option<&str>,
    extra_parts: &[Part],
) -> JsonRpcRequest {
    let ctx = context_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut parts = vec![text_part(text)];
    parts.extend(extra_parts.iter().cloned());

    let message = Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(ctx),
        task_id: None,
        role: Role::User,
        parts,
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };

    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: JsonRpcId::String(uuid::Uuid::new_v4().to_string()),
        method: method.into(),
        params: Some(
            serde_json::to_value(&SendMessageRequest {
                message,
                configuration: None,
                metadata: None,
                tenant: None,
            })
            .unwrap(),
        ),
    }
}

/// Extract the transport path from an AgentCard JSON value.
///
/// Prefers the JSONRPC binding in `supportedInterfaces` (A2A ≥1.0), falling
/// back to the first declared interface, then to a legacy top-level `url`
/// (A2A 0.2.x cards). The A2A spec fixes no path — it must be read from the
/// card, never assumed (e.g. Nasiko's Rust agents mount at "/jsonrpc" while
/// other frameworks commonly serve at "/").
///
/// Returns a normalized path with no trailing slash ("/" for root).
pub fn extract_transport_path(card: &serde_json::Value) -> Option<String> {
    let iface_url = card
        .get("supportedInterfaces")
        .and_then(|v| v.as_array())
        .and_then(|ifaces| {
            ifaces
                .iter()
                .find(|i| {
                    i.get("protocolBinding")
                        .and_then(|p| p.as_str())
                        .is_some_and(|p| p.eq_ignore_ascii_case("JSONRPC"))
                })
                .or_else(|| ifaces.first())
        })
        .and_then(|i| i.get("url"))
        .and_then(|u| u.as_str())
        .or_else(|| card.get("url").and_then(|u| u.as_str()))?;

    let path = if let Some(rest) = iface_url
        .strip_prefix("http://")
        .or_else(|| iface_url.strip_prefix("https://"))
    {
        rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
    } else if iface_url.starts_with('/') {
        iface_url
    } else {
        return None;
    };

    let trimmed = path.trim_end_matches('/');
    Some(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

#[cfg(test)]
mod sse_event_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artifact_chunks_concatenate_without_newlines() {
        let ev = json!({"result": {"artifactUpdate": {"artifact": {
            "parts": [{"text": "I"}, {"text": "'ll"}, {"text": " start"}]
        }}}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::ArtifactText("I'll start".into())]
        );
    }

    #[test]
    fn bare_message_reply_is_terminal_with_text() {
        // a2a-go SDK agents answer a stream request with a single message event.
        let ev = json!({"jsonrpc": "2.0", "id": "1", "result": {"message": {
            "messageId": "m1",
            "role": "ROLE_AGENT",
            "parts": [{"text": "Weather for Tokyo"}, {"text": ": sunny"}]
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::Completed {
                snapshot_text: Some("Weather for Tokyo: sunny".into())
            }]
        );
    }

    #[test]
    fn working_status_text_and_data_parts_classify_separately() {
        let ev = json!({"statusUpdate": {"status": {
            "state": "TASK_STATE_WORKING",
            "message": {"parts": [
                {"text": "web_search: nasiko ssl"},
                {"data": {"type": "tool_call", "agent": "x"}}
            ]}
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![
                SseEvent::StatusText("web_search: nasiko ssl".into()),
                SseEvent::StatusData(json!({"type": "tool_call", "agent": "x"})),
            ]
        );
    }

    #[test]
    fn blank_status_text_is_dropped() {
        let ev = json!({"statusUpdate": {"status": {
            "state": "TASK_STATE_WORKING",
            "message": {"parts": [{"text": "  "}]}
        }}});
        assert_eq!(classify_sse_event(&ev), vec![]);
    }

    #[test]
    fn failed_status_carries_reason() {
        let ev = json!({"result": {"statusUpdate": {"status": {
            "state": "TASK_STATE_FAILED",
            "message": {"parts": [{"text": "boom"}]}
        }}}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::Failed {
                reason: "boom".into()
            }]
        );
    }

    #[test]
    fn completed_task_snapshot_yields_text() {
        let ev = json!({"result": {"task": {
            "artifacts": [{"parts": [{"text": "Hello."}]}],
            "status": {"state": "TASK_STATE_COMPLETED"}
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::Completed {
                snapshot_text: Some("Hello.".into())
            }]
        );
    }

    #[test]
    fn non_terminal_task_echo_is_ignored() {
        let ev = json!({"task": {"status": {"state": "TASK_STATE_SUBMITTED"}}});
        assert_eq!(classify_sse_event(&ev), vec![]);
    }

    #[test]
    fn legacy_kind_tagged_shapes_classify() {
        let art = json!({"kind": "artifact-update", "parts": [{"text": "hi"}]});
        assert_eq!(
            classify_sse_event(&art),
            vec![SseEvent::ArtifactText("hi".into())]
        );
        let done = json!({"kind": "status-update", "status": {"state": "completed"}});
        assert_eq!(
            classify_sse_event(&done),
            vec![SseEvent::Completed {
                snapshot_text: None
            }]
        );
    }
}

#[cfg(test)]
mod transport_path_tests {
    use super::extract_text;
    use super::extract_transport_path;
    use serde_json::json;

    #[test]
    fn prefers_jsonrpc_interface() {
        let card = json!({
            "supportedInterfaces": [
                { "url": "grpc://host:9000", "protocolBinding": "GRPC" },
                { "url": "http://0.0.0.0:9100/jsonrpc", "protocolBinding": "JSONRPC" }
            ]
        });
        assert_eq!(extract_transport_path(&card).as_deref(), Some("/jsonrpc"));
    }

    #[test]
    fn falls_back_to_first_interface_then_legacy_url() {
        let card = json!({
            "supportedInterfaces": [{ "url": "https://a.example/a2a/", "protocolBinding": "HTTP+JSON" }]
        });
        assert_eq!(extract_transport_path(&card).as_deref(), Some("/a2a"));

        let legacy = json!({ "url": "http://agent:8000/" });
        assert_eq!(extract_transport_path(&legacy).as_deref(), Some("/"));
    }

    #[test]
    fn handles_bare_paths_and_missing_data() {
        assert_eq!(
            extract_transport_path(&json!({ "url": "/jsonrpc" })).as_deref(),
            Some("/jsonrpc")
        );
        assert_eq!(extract_transport_path(&json!({})), None);
        assert_eq!(extract_transport_path(&json!({ "url": "not-a-url" })), None);
    }

    #[test]
    fn extract_text_merges_same_artifact_chunks_without_newlines() {
        // Streaming agents accumulate one artifact entry per chunk, all
        // sharing an artifactId — they are pieces of one reply, not lines.
        let result = json!({ "task": { "artifacts": [
            { "artifactId": "a1", "parts": [{ "text": "Hello" }] },
            { "artifactId": "a1", "parts": [{ "text": " world" }] },
            { "artifactId": "a1", "parts": [{ "text": "." }] },
        ]}});
        assert_eq!(extract_text(&result).as_deref(), Some("Hello world."));
    }

    #[test]
    fn extract_text_separates_distinct_artifacts_with_newline() {
        let result = json!({ "artifacts": [
            { "artifactId": "a1", "parts": [{ "text": "first" }] },
            { "artifactId": "a2", "parts": [{ "text": "second" }] },
        ]});
        assert_eq!(extract_text(&result).as_deref(), Some("first\nsecond"));
    }
}
