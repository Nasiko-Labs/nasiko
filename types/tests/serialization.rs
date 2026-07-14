/// Serialization round-trip and JSON shape tests for nasiko-types.
///
/// Covers every public type in `nasiko_types::a2a` and `nasiko_types::registry`.
/// Tests verify:
///   1. Round-trip: serialize → deserialize → equal
///   2. JSON shape: field names match A2A spec (camelCase where required)
///   3. Enum string values match the A2A wire format
use nasiko_types::a2a::{
    Artifact, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, Message, Part, PartContent,
    Role, SendMessageRequest, StreamResponse, Task, TaskArtifactUpdateEvent, TaskState,
    TaskStatus, TaskStatusUpdateEvent, agent_message, artifact_event, build_send_request,
    build_stream_request, completed, data_part, extract_text, failed, status_event, task_event,
    text_chunk, text_part, to_sse_data, working, working_with_message,
};
use nasiko_types::registry::{
    Artifact as RegistryArtifact, ArtifactResponse, PublishRequest, PublishResponse,
    SearchResponse, VersionsResponse,
};
use serde_json::{Value, json};

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn make_artifact() -> Artifact {
    Artifact {
        artifact_id: "art-1".into(),
        name: Some("output".into()),
        description: Some("test artifact".into()),
        parts: vec![text_part("hello")],
        metadata: None,
        extensions: None,
    }
}

fn make_task_status(state: TaskState) -> TaskStatus {
    TaskStatus { state, message: None, timestamp: None }
}

fn make_task() -> Task {
    Task {
        id: "task-1".into(),
        context_id: "ctx-1".into(),
        status: make_task_status(TaskState::Working),
        artifacts: None,
        history: None,
        metadata: None,
    }
}

fn make_message(role: Role) -> Message {
    Message {
        message_id: "msg-1".into(),
        context_id: Some("ctx-1".into()),
        task_id: Some("task-1".into()),
        role,
        parts: vec![text_part("hello world")],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    }
}

// ─── Role ────────────────────────────────────────────────────────────────────

#[test]
fn role_user_serializes_to_correct_string() {
    let v = serde_json::to_value(Role::User).unwrap();
    assert_eq!(v, json!("ROLE_USER"));
}

#[test]
fn role_agent_serializes_to_correct_string() {
    let v = serde_json::to_value(Role::Agent).unwrap();
    assert_eq!(v, json!("ROLE_AGENT"));
}

#[test]
fn role_unspecified_serializes_to_correct_string() {
    let v = serde_json::to_value(Role::Unspecified).unwrap();
    assert_eq!(v, json!("ROLE_UNSPECIFIED"));
}

#[test]
fn role_round_trips() {
    for role in [Role::User, Role::Agent, Role::Unspecified] {
        let json = serde_json::to_string(&role).unwrap();
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(back, role);
    }
}

#[test]
fn role_deserializes_from_string() {
    assert_eq!(serde_json::from_str::<Role>(r#""ROLE_USER""#).unwrap(), Role::User);
    assert_eq!(serde_json::from_str::<Role>(r#""ROLE_AGENT""#).unwrap(), Role::Agent);
    assert_eq!(serde_json::from_str::<Role>(r#""ROLE_UNSPECIFIED""#).unwrap(), Role::Unspecified);
}

// ─── TaskState ────────────────────────────────────────────────────────────────

#[test]
fn task_state_variants_serialize_to_correct_strings() {
    let cases = [
        (TaskState::Unspecified, "TASK_STATE_UNSPECIFIED"),
        (TaskState::Submitted, "TASK_STATE_SUBMITTED"),
        (TaskState::Working, "TASK_STATE_WORKING"),
        (TaskState::Completed, "TASK_STATE_COMPLETED"),
        (TaskState::Failed, "TASK_STATE_FAILED"),
        (TaskState::Canceled, "TASK_STATE_CANCELED"),
        (TaskState::InputRequired, "TASK_STATE_INPUT_REQUIRED"),
        (TaskState::Rejected, "TASK_STATE_REJECTED"),
        (TaskState::AuthRequired, "TASK_STATE_AUTH_REQUIRED"),
    ];
    for (state, expected) in cases {
        let v = serde_json::to_value(&state).unwrap();
        assert_eq!(v, json!(expected), "TaskState::{state:?} should serialize to {expected}");
    }
}

#[test]
fn task_state_round_trips() {
    let states = [
        TaskState::Unspecified,
        TaskState::Submitted,
        TaskState::Working,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Canceled,
        TaskState::InputRequired,
        TaskState::Rejected,
        TaskState::AuthRequired,
    ];
    for state in states {
        let json = serde_json::to_string(&state).unwrap();
        let back: TaskState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, state);
    }
}

#[test]
fn task_state_is_terminal_for_correct_variants() {
    assert!(TaskState::Completed.is_terminal());
    assert!(TaskState::Failed.is_terminal());
    assert!(TaskState::Canceled.is_terminal());
    assert!(TaskState::Rejected.is_terminal());
    assert!(!TaskState::Working.is_terminal());
    assert!(!TaskState::Submitted.is_terminal());
    assert!(!TaskState::InputRequired.is_terminal());
}

// ─── Part ────────────────────────────────────────────────────────────────────

#[test]
fn text_part_serializes_with_text_field() {
    let p = text_part("hello");
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v.get("text").and_then(|v| v.as_str()), Some("hello"));
    // No content wrapper field — the text IS the top-level field
    assert!(v.get("content").is_none());
}

#[test]
fn data_part_serializes_with_data_field() {
    let p = data_part(json!({"key": "value"}));
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v.get("data"), Some(&json!({"key": "value"})));
}

#[test]
fn part_with_filename_and_media_type_round_trips() {
    let p = Part {
        content: PartContent::Text("content".into()),
        filename: Some("file.txt".into()),
        media_type: Some("text/plain".into()),
        metadata: None,
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: Part = serde_json::from_str(&json).unwrap();
    assert_eq!(back.filename, Some("file.txt".into()));
    assert_eq!(back.media_type, Some("text/plain".into()));
    match back.content {
        PartContent::Text(t) => assert_eq!(t, "content"),
        other => panic!("Expected Text, got {other:?}"),
    }
}

#[test]
fn part_media_type_serializes_as_camel_case() {
    let p = Part {
        content: PartContent::Text("x".into()),
        filename: None,
        media_type: Some("application/json".into()),
        metadata: None,
    };
    let v = serde_json::to_value(&p).unwrap();
    // The a2a spec uses camelCase: "mediaType"
    assert!(v.get("mediaType").is_some(), "mediaType field should use camelCase in JSON");
    assert!(v.get("media_type").is_none(), "snake_case 'media_type' should not appear");
}

#[test]
fn part_url_content_round_trips() {
    let p = Part {
        content: PartContent::Url("https://example.com/file".into()),
        filename: None,
        media_type: None,
        metadata: None,
    };
    let json = serde_json::to_string(&p).unwrap();
    let back: Part = serde_json::from_str(&json).unwrap();
    match back.content {
        PartContent::Url(u) => assert_eq!(u, "https://example.com/file"),
        other => panic!("Expected Url, got {other:?}"),
    }
}

#[test]
fn part_raw_content_round_trips_via_base64() {
    let bytes = vec![1u8, 2, 3, 0xff];
    let p = Part {
        content: PartContent::Raw(bytes.clone()),
        filename: None,
        media_type: None,
        metadata: None,
    };
    let json = serde_json::to_string(&p).unwrap();
    let v: Value = serde_json::from_str(&json).unwrap();
    // raw field should be present and be a base64 string
    assert!(v.get("raw").and_then(|v| v.as_str()).is_some());
    let back: Part = serde_json::from_str(&json).unwrap();
    match back.content {
        PartContent::Raw(b) => assert_eq!(b, bytes),
        other => panic!("Expected Raw, got {other:?}"),
    }
}

// ─── Message ─────────────────────────────────────────────────────────────────

#[test]
fn message_round_trips() {
    let msg = make_message(Role::User);
    let json = serde_json::to_string(&msg).unwrap();
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back.message_id, "msg-1");
    assert_eq!(back.role, Role::User);
    assert_eq!(back.parts.len(), 1);
    assert_eq!(back.context_id, Some("ctx-1".into()));
    assert_eq!(back.task_id, Some("task-1".into()));
}

#[test]
fn message_uses_camel_case_field_names() {
    let msg = make_message(Role::Agent);
    let v = serde_json::to_value(&msg).unwrap();
    // A2A spec uses camelCase
    assert!(v.get("messageId").is_some(), "messageId field should be camelCase");
    assert!(v.get("contextId").is_some(), "contextId field should be camelCase");
    assert!(v.get("taskId").is_some(), "taskId field should be camelCase");
    assert!(v.get("message_id").is_none(), "snake_case should not appear");
}

#[test]
fn message_optional_fields_omitted_when_none() {
    let msg = Message {
        message_id: "m1".into(),
        context_id: None,
        task_id: None,
        role: Role::User,
        parts: vec![text_part("hi")],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let v = serde_json::to_value(&msg).unwrap();
    assert!(v.get("contextId").is_none(), "None contextId should be omitted");
    assert!(v.get("taskId").is_none(), "None taskId should be omitted");
    assert!(v.get("metadata").is_none(), "None metadata should be omitted");
}

// ─── TaskStatus ───────────────────────────────────────────────────────────────

#[test]
fn task_status_round_trips() {
    let status = TaskStatus {
        state: TaskState::Completed,
        message: Some(make_message(Role::Agent)),
        timestamp: None,
    };
    let json = serde_json::to_string(&status).unwrap();
    let back: TaskStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back.state, TaskState::Completed);
    assert!(back.message.is_some());
}

#[test]
fn task_status_none_message_is_omitted() {
    let status = make_task_status(TaskState::Working);
    let v = serde_json::to_value(&status).unwrap();
    assert!(v.get("message").is_none(), "None message should be omitted from JSON");
}

// ─── Task ────────────────────────────────────────────────────────────────────

#[test]
fn task_round_trips() {
    let task = make_task();
    let json = serde_json::to_string(&task).unwrap();
    let back: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "task-1");
    assert_eq!(back.context_id, "ctx-1");
    assert_eq!(back.status.state, TaskState::Working);
}

#[test]
fn task_uses_camel_case_field_names() {
    let task = make_task();
    let v = serde_json::to_value(&task).unwrap();
    assert!(v.get("contextId").is_some(), "contextId field should be camelCase");
    assert!(v.get("context_id").is_none(), "snake_case should not appear");
}

#[test]
fn task_with_artifacts_round_trips() {
    let mut task = make_task();
    task.artifacts = Some(vec![make_artifact()]);
    let json = serde_json::to_string(&task).unwrap();
    let back: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(back.artifacts.as_ref().map(|a| a.len()), Some(1));
    assert_eq!(back.artifacts.unwrap()[0].artifact_id, "art-1");
}

// ─── Artifact (a2a) ──────────────────────────────────────────────────────────

#[test]
fn a2a_artifact_round_trips() {
    let art = make_artifact();
    let json = serde_json::to_string(&art).unwrap();
    let back: Artifact = serde_json::from_str(&json).unwrap();
    assert_eq!(back.artifact_id, "art-1");
    assert_eq!(back.name, Some("output".into()));
    assert_eq!(back.parts.len(), 1);
}

#[test]
fn a2a_artifact_uses_camel_case() {
    let art = make_artifact();
    let v = serde_json::to_value(&art).unwrap();
    assert!(v.get("artifactId").is_some(), "artifactId field should be camelCase");
    assert!(v.get("artifact_id").is_none(), "snake_case should not appear");
}

#[test]
fn a2a_artifact_optional_fields_omitted_when_none() {
    let art = Artifact {
        artifact_id: "a1".into(),
        name: None,
        description: None,
        parts: vec![text_part("x")],
        metadata: None,
        extensions: None,
    };
    let v = serde_json::to_value(&art).unwrap();
    assert!(v.get("name").is_none());
    assert!(v.get("description").is_none());
    assert!(v.get("metadata").is_none());
    assert!(v.get("extensions").is_none());
}

// ─── TaskStatusUpdateEvent ────────────────────────────────────────────────────

#[test]
fn task_status_update_event_round_trips() {
    let event = TaskStatusUpdateEvent {
        task_id: "t1".into(),
        context_id: "ctx-1".into(),
        status: make_task_status(TaskState::Completed),
        metadata: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: TaskStatusUpdateEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back.task_id, "t1");
    assert_eq!(back.context_id, "ctx-1");
    assert_eq!(back.status.state, TaskState::Completed);
}

#[test]
fn task_status_update_event_uses_camel_case() {
    let event = TaskStatusUpdateEvent {
        task_id: "t1".into(),
        context_id: "ctx-1".into(),
        status: make_task_status(TaskState::Working),
        metadata: None,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert!(v.get("taskId").is_some());
    assert!(v.get("contextId").is_some());
    assert!(v.get("task_id").is_none());
}

// ─── TaskArtifactUpdateEvent ──────────────────────────────────────────────────

#[test]
fn task_artifact_update_event_round_trips() {
    let event = TaskArtifactUpdateEvent {
        task_id: "t1".into(),
        context_id: "ctx-1".into(),
        artifact: make_artifact(),
        append: Some(true),
        last_chunk: Some(false),
        metadata: None,
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: TaskArtifactUpdateEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(back.task_id, "t1");
    assert_eq!(back.append, Some(true));
    assert_eq!(back.last_chunk, Some(false));
}

#[test]
fn task_artifact_update_event_optional_fields_omitted_when_none() {
    let event = TaskArtifactUpdateEvent {
        task_id: "t1".into(),
        context_id: "c1".into(),
        artifact: make_artifact(),
        append: None,
        last_chunk: None,
        metadata: None,
    };
    let v = serde_json::to_value(&event).unwrap();
    assert!(v.get("append").is_none());
    assert!(v.get("lastChunk").is_none());
}

// ─── StreamResponse ───────────────────────────────────────────────────────────

#[test]
fn stream_response_task_serializes_with_task_key() {
    let r = task_event(make_task());
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("task").is_some(), "StreamResponse::Task should serialize to {{\"task\": ...}}");
    assert!(v.get("statusUpdate").is_none());
    assert!(v.get("artifactUpdate").is_none());
}

#[test]
fn stream_response_task_round_trips() {
    let r = task_event(make_task());
    let json = serde_json::to_string(&r).unwrap();
    let back: StreamResponse = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, StreamResponse::Task(_)));
}

#[test]
fn stream_response_status_update_serializes_with_status_update_key() {
    let event = TaskStatusUpdateEvent {
        task_id: "t1".into(),
        context_id: "c1".into(),
        status: make_task_status(TaskState::Working),
        metadata: None,
    };
    let r = status_event(event);
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("statusUpdate").is_some(), "StatusUpdate should use camelCase key 'statusUpdate'");
    assert!(v.get("task").is_none());
}

#[test]
fn stream_response_status_update_round_trips() {
    let event = TaskStatusUpdateEvent {
        task_id: "t1".into(),
        context_id: "c1".into(),
        status: make_task_status(TaskState::Completed),
        metadata: None,
    };
    let r = status_event(event);
    let json = serde_json::to_string(&r).unwrap();
    let back: StreamResponse = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, StreamResponse::StatusUpdate(_)));
}

#[test]
fn stream_response_artifact_update_serializes_with_artifact_update_key() {
    let event = TaskArtifactUpdateEvent {
        task_id: "t1".into(),
        context_id: "c1".into(),
        artifact: make_artifact(),
        append: None,
        last_chunk: None,
        metadata: None,
    };
    let r = artifact_event(event);
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("artifactUpdate").is_some(), "ArtifactUpdate should use 'artifactUpdate' key");
}

#[test]
fn stream_response_artifact_update_round_trips() {
    let event = TaskArtifactUpdateEvent {
        task_id: "t1".into(),
        context_id: "c1".into(),
        artifact: make_artifact(),
        append: Some(true),
        last_chunk: Some(true),
        metadata: None,
    };
    let r = artifact_event(event);
    let json = serde_json::to_string(&r).unwrap();
    let back: StreamResponse = serde_json::from_str(&json).unwrap();
    assert!(matches!(back, StreamResponse::ArtifactUpdate(_)));
}

#[test]
fn stream_response_unknown_variant_fails_to_deserialize() {
    let json = r#"{"unknown": {}}"#;
    let result = serde_json::from_str::<StreamResponse>(json);
    assert!(result.is_err(), "Unknown StreamResponse variant should fail to deserialize");
}

// ─── JsonRpcId ────────────────────────────────────────────────────────────────

#[test]
fn jsonrpc_id_string_round_trips() {
    let id = JsonRpcId::String("abc-123".into());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, r#""abc-123""#);
    let back: JsonRpcId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, JsonRpcId::String("abc-123".into()));
}

#[test]
fn jsonrpc_id_number_round_trips() {
    let id = JsonRpcId::Number(42);
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "42");
    let back: JsonRpcId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, JsonRpcId::Number(42));
}

#[test]
fn jsonrpc_id_null_round_trips() {
    let id = JsonRpcId::Null;
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "null");
    let back: JsonRpcId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, JsonRpcId::Null);
}

// ─── JsonRpcRequest ───────────────────────────────────────────────────────────

#[test]
fn jsonrpc_request_round_trips() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: JsonRpcId::String("req-1".into()),
        method: "SendMessage".into(),
        params: Some(json!({"message": "hello"})),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: JsonRpcRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.jsonrpc, "2.0");
    assert_eq!(back.method, "SendMessage");
    assert_eq!(back.id, JsonRpcId::String("req-1".into()));
}

#[test]
fn jsonrpc_request_params_none_is_omitted() {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: JsonRpcId::Null,
        method: "health".into(),
        params: None,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert!(v.get("params").is_none(), "None params should be omitted from JSON");
}

// ─── JsonRpcResponse ─────────────────────────────────────────────────────────

#[test]
fn jsonrpc_response_success_round_trips() {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: JsonRpcId::Number(1),
        result: Some(json!({"status": "ok"})),
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
    assert!(back.result.is_some());
    assert!(back.error.is_none());
    assert_eq!(back.result.unwrap(), json!({"status": "ok"}));
}

#[test]
fn jsonrpc_response_error_round_trips() {
    let err = JsonRpcError { code: -32600, message: "Invalid Request".into(), data: None };
    let resp = JsonRpcResponse {
        jsonrpc: "2.0".into(),
        id: JsonRpcId::Null,
        result: None,
        error: Some(err),
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: JsonRpcResponse = serde_json::from_str(&json).unwrap();
    assert!(back.error.is_some());
    assert!(back.result.is_none());
    assert_eq!(back.error.unwrap().code, -32600);
}

// ─── JsonRpcError ─────────────────────────────────────────────────────────────

#[test]
fn jsonrpc_error_round_trips_with_data() {
    let err = JsonRpcError {
        code: -32700,
        message: "Parse error".into(),
        data: Some(json!({"detail": "unexpected token"})),
    };
    let json = serde_json::to_string(&err).unwrap();
    let back: JsonRpcError = serde_json::from_str(&json).unwrap();
    assert_eq!(back.code, -32700);
    assert_eq!(back.message, "Parse error");
    assert_eq!(back.data, Some(json!({"detail": "unexpected token"})));
}

#[test]
fn jsonrpc_error_none_data_omitted() {
    let err = JsonRpcError { code: -32600, message: "Invalid".into(), data: None };
    let v = serde_json::to_value(&err).unwrap();
    assert!(v.get("data").is_none());
}

// ─── Constructor helpers ──────────────────────────────────────────────────────

#[test]
fn build_send_request_has_correct_method_and_version() {
    let req = build_send_request("hello", Some("ctx-1"));
    assert_eq!(req.jsonrpc, "2.0");
    // Current A2A JSON-RPC spec naming (matches what every example agent's
    // `a2a-sdk` pin actually accepts), not the gRPC-style `SendMessage`.
    assert_eq!(req.method, "message/send");
    assert!(req.params.is_some());
}

#[test]
fn build_stream_request_has_correct_method() {
    let req = build_stream_request("hello", None);
    assert_eq!(req.method, "message/stream");
}

#[test]
fn build_send_request_params_have_lowercase_role_and_correct_text() {
    // Deliberately asserted against the raw `Value`, not deserialized back into
    // `SendMessageRequest`/`Role` — those (from the external `a2a-lf` crate) only
    // accept the gRPC-style `"ROLE_USER"`, but this request is sent outbound to
    // agent containers whose `a2a-sdk` (Python) only accepts current-spec
    // lowercase `"user"`, which is what `build_request` deliberately patches in.
    let req = build_send_request("test text", Some("ctx-abc"));
    let params = req.params.unwrap();
    assert_eq!(params["message"]["role"], json!("user"));
    assert_eq!(params["message"]["parts"][0]["text"], json!("test text"));
}

#[test]
fn working_constructor_produces_working_state() {
    let event = working("t1", "c1");
    assert_eq!(event.task_id, "t1");
    assert_eq!(event.context_id, "c1");
    assert_eq!(event.status.state, TaskState::Working);
    assert!(event.status.message.is_none());
}

#[test]
fn completed_constructor_produces_completed_state() {
    let event = completed("t2", "c2");
    assert_eq!(event.status.state, TaskState::Completed);
}

#[test]
fn failed_constructor_produces_failed_state_with_error_message() {
    let event = failed("t3", "c3", "something went wrong");
    assert_eq!(event.status.state, TaskState::Failed);
    assert!(event.status.message.is_some());
    let msg = event.status.message.unwrap();
    assert_eq!(msg.role, Role::Agent);
    assert!(!msg.parts.is_empty());
    match &msg.parts[0].content {
        PartContent::Text(t) => assert_eq!(t, "something went wrong"),
        other => panic!("Expected Text, got {other:?}"),
    }
}

#[test]
fn working_with_message_constructor_attaches_message() {
    let msg = make_message(Role::Agent);
    let event = working_with_message("t1", "c1", msg.clone());
    assert_eq!(event.status.state, TaskState::Working);
    assert!(event.status.message.is_some());
    assert_eq!(event.status.message.unwrap().message_id, msg.message_id);
}

#[test]
fn agent_message_constructor_produces_agent_role() {
    let msg = agent_message("ctx-1", "task-1", text_part("response"));
    assert_eq!(msg.role, Role::Agent);
    assert_eq!(msg.context_id, Some("ctx-1".into()));
    assert_eq!(msg.task_id, Some("task-1".into()));
    assert!(!msg.message_id.is_empty());
}

#[test]
fn text_chunk_constructor_round_trips() {
    let ev = text_chunk("t1", "c1", "a1", "chunk text", true, false);
    assert_eq!(ev.task_id, "t1");
    assert_eq!(ev.context_id, "c1");
    assert_eq!(ev.artifact.artifact_id, "a1");
    assert_eq!(ev.append, Some(true));
    assert_eq!(ev.last_chunk, Some(false));
    match &ev.artifact.parts[0].content {
        PartContent::Text(t) => assert_eq!(t, "chunk text"),
        other => panic!("Expected Text, got {other:?}"),
    }
}

#[test]
fn to_sse_data_returns_valid_json_string() {
    let r = task_event(make_task());
    let s = to_sse_data(&r);
    let v: Value = serde_json::from_str(&s).expect("to_sse_data should return valid JSON");
    assert!(v.get("task").is_some());
}

// ─── extract_text ─────────────────────────────────────────────────────────────

#[test]
fn extract_text_from_artifacts_array() {
    let result = json!({
        "artifacts": [
            {
                "parts": [{"text": "part one"}]
            },
            {
                "parts": [{"text": "part two"}]
            }
        ]
    });
    let text = extract_text(&result);
    assert!(text.is_some());
    let t = text.unwrap();
    assert!(t.contains("part one"));
    assert!(t.contains("part two"));
}

#[test]
fn extract_text_from_task_wrapper() {
    let result = json!({
        "task": {
            "artifacts": [
                {"parts": [{"text": "wrapped text"}]}
            ]
        }
    });
    let text = extract_text(&result);
    assert_eq!(text, Some("wrapped text".to_string()));
}

#[test]
fn extract_text_from_status_message_parts() {
    let result = json!({
        "status": {
            "message": {
                "parts": [{"text": "status text"}]
            }
        }
    });
    let text = extract_text(&result);
    assert_eq!(text, Some("status text".to_string()));
}

#[test]
fn extract_text_from_message_parts() {
    let result = json!({
        "message": {
            "parts": [{"text": "message text"}]
        }
    });
    let text = extract_text(&result);
    assert_eq!(text, Some("message text".to_string()));
}

#[test]
fn extract_text_returns_none_when_no_text() {
    let result = json!({"other": "field"});
    assert!(extract_text(&result).is_none());
}

// ─── Registry types ───────────────────────────────────────────────────────────

fn make_registry_artifact() -> RegistryArtifact {
    RegistryArtifact {
        id: "reg-1".into(),
        owner: "nasiko".into(),
        name: "coding-agent".into(),
        version: "1.0.0".into(),
        artifact_type: "agent".into(),
        status: "published".into(),
        description: Some("A coding agent".into()),
        oci_digest: Some("sha256:abc123".into()),
        size_bytes: Some(1024),
        tags: vec!["rust".into(), "coding".into()],
        framework: Some("nasiko".into()),
        license: Some("MIT".into()),
        created_at: Some("2024-01-01T00:00:00Z".into()),
        updated_at: Some("2024-01-02T00:00:00Z".into()),
        score: None,
    }
}

#[test]
fn registry_artifact_round_trips() {
    let art = make_registry_artifact();
    let json = serde_json::to_string(&art).unwrap();
    let back: RegistryArtifact = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "reg-1");
    assert_eq!(back.owner, "nasiko");
    assert_eq!(back.name, "coding-agent");
    assert_eq!(back.version, "1.0.0");
    assert_eq!(back.artifact_type, "agent");
    assert_eq!(back.status, "published");
    assert_eq!(back.tags, vec!["rust", "coding"]);
    assert_eq!(back.size_bytes, Some(1024));
}

#[test]
fn registry_artifact_score_omitted_when_none() {
    let art = make_registry_artifact();
    let v = serde_json::to_value(&art).unwrap();
    // score uses skip_serializing_if = "Option::is_none"
    assert!(v.get("score").is_none(), "score=None should be omitted from JSON");
}

#[test]
fn registry_artifact_score_included_when_some() {
    let mut art = make_registry_artifact();
    art.score = Some(0.95);
    let v = serde_json::to_value(&art).unwrap();
    let score = v.get("score").and_then(|s| s.as_f64());
    assert!(score.is_some());
    assert!((score.unwrap() - 0.95).abs() < 1e-6);
}

#[test]
fn registry_artifact_optional_fields_default_when_missing_from_json() {
    let json = r#"{
        "id": "r1",
        "owner": "owner",
        "name": "agent",
        "version": "0.1",
        "artifact_type": "agent",
        "status": "draft"
    }"#;
    let art: RegistryArtifact = serde_json::from_str(json).unwrap();
    assert_eq!(art.description, None);
    assert_eq!(art.oci_digest, None);
    assert_eq!(art.size_bytes, None);
    assert!(art.tags.is_empty());
    assert_eq!(art.framework, None);
    assert_eq!(art.score, None);
}

#[test]
fn publish_request_round_trips() {
    let req = PublishRequest {
        owner: "nasiko".into(),
        name: "agent".into(),
        version: "2.0.0".into(),
        artifact_type: "agent".into(),
        description: Some("desc".into()),
        tags: vec!["tag1".into()],
        framework: Some("nasiko".into()),
        license: Some("Apache-2.0".into()),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: PublishRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.owner, "nasiko");
    assert_eq!(back.version, "2.0.0");
    assert_eq!(back.tags, vec!["tag1"]);
}

#[test]
fn publish_request_optional_fields_default_when_missing() {
    let json = r#"{
        "owner": "o",
        "name": "n",
        "version": "1.0",
        "artifact_type": "skill"
    }"#;
    let req: PublishRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.description, None);
    assert!(req.tags.is_empty());
    assert_eq!(req.framework, None);
    assert_eq!(req.license, None);
}

#[test]
fn publish_response_round_trips() {
    let resp = PublishResponse {
        artifact: make_registry_artifact(),
        upload_url: "https://s3.example.com/upload/123".into(),
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: PublishResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.artifact.id, "reg-1");
    assert_eq!(back.upload_url, "https://s3.example.com/upload/123");
}

#[test]
fn search_response_round_trips() {
    let resp = SearchResponse {
        data: vec![make_registry_artifact()],
        total: Some(1),
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: SearchResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.data.len(), 1);
    assert_eq!(back.total, Some(1));
}

#[test]
fn search_response_total_defaults_to_none() {
    let json = r#"{"data": []}"#;
    let resp: SearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.total, None);
    assert!(resp.data.is_empty());
}

#[test]
fn artifact_response_round_trips() {
    let resp = ArtifactResponse { data: make_registry_artifact() };
    let json = serde_json::to_string(&resp).unwrap();
    let back: ArtifactResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.data.id, "reg-1");
    assert_eq!(back.data.name, "coding-agent");
}

#[test]
fn versions_response_round_trips() {
    let resp = VersionsResponse {
        data: vec![make_registry_artifact()],
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: VersionsResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(back.data.len(), 1);
    assert_eq!(back.data[0].version, "1.0.0");
}

#[test]
fn versions_response_empty_list_round_trips() {
    let resp = VersionsResponse { data: vec![] };
    let json = serde_json::to_string(&resp).unwrap();
    let back: VersionsResponse = serde_json::from_str(&json).unwrap();
    assert!(back.data.is_empty());
}

// ─── SendMessageRequest ───────────────────────────────────────────────────────

#[test]
fn send_message_request_round_trips() {
    let req = SendMessageRequest {
        message: make_message(Role::User),
        configuration: None,
        metadata: None,
        tenant: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: SendMessageRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.message.role, Role::User);
    assert_eq!(back.message.message_id, "msg-1");
}

#[test]
fn send_message_request_optional_fields_omitted_when_none() {
    let req = SendMessageRequest {
        message: make_message(Role::User),
        configuration: None,
        metadata: None,
        tenant: None,
    };
    let v = serde_json::to_value(&req).unwrap();
    assert!(v.get("configuration").is_none());
    assert!(v.get("metadata").is_none());
}
