//! Thin wrapper around the official `a2a` crate (a2a-lf) with nasiko-specific helpers.

pub use a2a::{
    Artifact, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, Message, Part,
    PartContent, Role, SendMessageConfiguration, SendMessageRequest, StreamResponse, Task,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent, new_artifact_id,
    new_context_id, new_message_id, new_task_id,
};

// ─── Part constructors ──────────────────────────────────────────────────────

pub fn text_part(s: impl Into<String>) -> Part {
    Part { content: PartContent::Text(s.into()), filename: None, media_type: None, metadata: None }
}

pub fn data_part(value: serde_json::Value) -> Part {
    Part { content: PartContent::Data(value), filename: None, media_type: None, metadata: None }
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

pub fn working_with_message(task_id: &str, context_id: &str, msg: Message) -> TaskStatusUpdateEvent {
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
    build_request("SendMessage", text, context_id)
}

pub fn build_stream_request(text: &str, context_id: Option<&str>) -> JsonRpcRequest {
    build_request("SendStreamingMessage", text, context_id)
}

pub fn build_stream_request_with_metadata(
    text: &str,
    context_id: Option<&str>,
    metadata: serde_json::Value,
) -> JsonRpcRequest {
    let mut req = build_request("SendStreamingMessage", text, context_id);
    if let Some(params) = req.params.as_mut()
        && let Some(obj) = params.as_object_mut() {
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

    if let Some(artifacts) = task.get("artifacts").and_then(|a| a.as_array()) {
        let mut texts = Vec::new();
        for artifact in artifacts {
            if let Some(parts) = artifact.get("parts").and_then(|p| p.as_array()) {
                for part in parts {
                    if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                        texts.push(t);
                    }
                }
            }
        }
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }

    if let Some(parts) = task
        .pointer("/status/message/parts")
        .and_then(|p| p.as_array())
    {
        let texts: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }

    if let Some(parts) = result
        .pointer("/message/parts")
        .and_then(|p| p.as_array())
    {
        let texts: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }

    None
}

pub fn extract_text_from_response(response: &JsonRpcResponse) -> Option<String> {
    extract_text(response.result.as_ref()?)
}

// ─── Private ────────────────────────────────────────────────────────────────

fn build_request(method: &str, text: &str, context_id: Option<&str>) -> JsonRpcRequest {
    let ctx = context_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let message = Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(ctx),
        task_id: None,
        role: Role::User,
        parts: vec![text_part(text)],
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
    Some(if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() })
}

#[cfg(test)]
mod transport_path_tests {
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
}
