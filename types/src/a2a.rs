//! Thin wrapper around the official `a2a` crate (a2a-lf) with nasiko-specific helpers.

use serde::Serialize;

pub use a2a::{
    Artifact, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, Message, Part, PartContent,
    Role, SendMessageConfiguration, SendMessageRequest, StreamResponse, Task,
    TaskArtifactUpdateEvent, TaskState, TaskStatus, TaskStatusUpdateEvent, new_artifact_id,
    new_context_id, new_message_id, new_task_id,
};

/// The `A2A-Version` header value every outbound A2A call in this codebase sends — ten call sites
/// across six crates hardcoded the literal `"1.0"` independently (found in review); this is the
/// shared source of truth. Consistent with the existing convention, so its absence wasn't a
/// regression, but a future protocol bump only needs one edit with this in place.
pub const A2A_VERSION_HEADER_VALUE: &str = "1.0";

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

pub fn input_required(task_id: &str, context_id: &str, message: &str) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::InputRequired,
            message: Some(agent_message(context_id, task_id, text_part(message))),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

pub fn auth_required(task_id: &str, context_id: &str, message: &str) -> TaskStatusUpdateEvent {
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::AuthRequired,
            message: Some(agent_message(context_id, task_id, text_part(message))),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    }
}

/// Terminal-for-this-turn status: a sub-agent paused and needs a human before the
/// conversation can continue. Mirrors `failed` (also terminal, also carries a message),
/// but maps to `TaskState::InputRequired`/`AuthRequired` instead of `Failed` — the client
/// must be able to tell "the agent needs input" apart from "the agent errored."
///
/// Distinct from `input_required`/`auth_required` above (from `feat/hitl-direct-chat`): those
/// build their own `Message` from a plain `&str`, for `agent_proxy`'s direct-chat path. This one
/// takes an already-built `Message` and a `kind`, for the orchestrator's `AwaitingHuman` event
/// (`a2a_dispatch.rs`'s `orchestrator_stream`), which already has a `Message` assembled with a
/// `data` part, not just plain text. Kept side by side rather than unified — each caller's
/// `Message` shape differs enough that forcing one signature would lose information at one
/// call site or the other.
pub fn awaiting_human(
    task_id: &str,
    context_id: &str,
    kind: AwaitingHumanKind,
    msg: Message,
) -> TaskStatusUpdateEvent {
    let state = match kind {
        AwaitingHumanKind::InputRequired => TaskState::InputRequired,
        AwaitingHumanKind::AuthRequired => TaskState::AuthRequired,
    };
    TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state,
            message: Some(msg),
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
    build_request("SendMessage", text, context_id, &[], None)
}

pub fn build_stream_request(text: &str, context_id: Option<&str>) -> JsonRpcRequest {
    build_request("SendStreamingMessage", text, context_id, &[], None)
}

pub fn build_stream_request_with_parts(
    text: &str,
    context_id: Option<&str>,
    extra_parts: &[Part],
) -> JsonRpcRequest {
    build_request("SendStreamingMessage", text, context_id, extra_parts, None)
}

pub fn build_stream_request_with_metadata(
    text: &str,
    context_id: Option<&str>,
    metadata: serde_json::Value,
) -> JsonRpcRequest {
    let mut req = build_request("SendStreamingMessage", text, context_id, &[], None);
    if let Some(params) = req.params.as_mut()
        && let Some(obj) = params.as_object_mut()
    {
        obj.insert("metadata".to_string(), metadata);
    }
    req
}

/// Continues an existing task rather than starting a new one — the only builder here that sets
/// `message.taskId`. Used exclusively by the HITL resume dispatcher (`oss/server/src/hitl`) to
/// send the human's answer back on the same `taskId`/`contextId` the agent paused on; every other
/// builder above deliberately omits `taskId` (a fresh task per call is today's live-chat
/// behavior).
pub fn build_stream_request_for_task(
    text: &str,
    context_id: &str,
    task_id: &str,
) -> JsonRpcRequest {
    build_request(
        "SendStreamingMessage",
        text,
        Some(context_id),
        &[],
        Some(task_id),
    )
}

/// Non-streaming counterpart to [`build_stream_request_for_task`] — used by the HITL resume
/// dispatcher's one-shot `message/send` retry when an agent's `message/stream` resume attempt
/// comes back with a JSON-RPC `error` (mirrors `a2a_dispatch.rs`'s own dispatch-time fallback).
pub fn build_send_request_for_task(text: &str, context_id: &str, task_id: &str) -> JsonRpcRequest {
    build_request("SendMessage", text, Some(context_id), &[], Some(task_id))
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

/// Which human-in-the-loop pause this is — mirrors `nasiko_hitl::HitlKind`'s two task-state
/// variants (`input_required`, `auth_required`). Duplicated here rather than depended on: this
/// crate has zero internal workspace dependencies, and `tool_approval` (hitl's third kind) never
/// applies at this layer — it isn't an A2A task state at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwaitingHumanKind {
    InputRequired,
    AuthRequired,
}

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
    /// Not terminal, but nothing can proceed without a human —
    /// `TASK_STATE_INPUT_REQUIRED` / `TASK_STATE_AUTH_REQUIRED`. Deliberately its own variant,
    /// never folded into `StatusText`: a caller that treated the agent's question as ordinary
    /// progress narration would feed it back to an LLM as if it were the answer.
    AwaitingHuman {
        kind: AwaitingHumanKind,
        message: String,
        metadata: serde_json::Value,
    },
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
            SseTaskState::AwaitingHuman(kind) => {
                out.push(SseEvent::AwaitingHuman {
                    kind,
                    message: awaiting_human_message(task),
                    metadata: awaiting_human_metadata(task),
                });
            }
            SseTaskState::Working | SseTaskState::Other => {}
        }
        return out;
    }
    // Flat A2A 0.3.x Task: no `task` wrapper at all — `result` itself IS the task
    // (`{"id":..., "kind": "task", "status": {...}}`), exactly what `a2a-lf` 0.3.0 and the
    // python/langgraph SDKs emit for a non-streaming pause. Without this the shape produces zero
    // events and a sub-agent's pause is silently handed back to the caller as a normal result.
    //
    // `kind` is required on a `Task` by the 0.3 spec, but it is only a discriminator — an emitter
    // that omits it still describes a pause, and `classify_stream_disposition`'s equivalent
    // fallback (`.unwrap_or(result)`) has no discriminator at all, so gating solely on `kind`
    // left the two classifiers disagreeing about the same bytes (found in review, probed: a
    // `kind`-less flat task returned `Paused` there and `[]` here). An absent `kind` plus a
    // `status.state` is therefore treated as a task too.
    //
    // Deliberately `kind.is_none()`, not "any kind that isn't task": a legacy kind-tagged
    // `status-update` also carries `status.state` and must keep falling through to
    // `classify_status` below, which additionally extracts `StatusData` from a
    // `working`/unknown-state payload's data parts — something this arm drops.
    let kind = result.get("kind").and_then(|k| k.as_str());
    if kind == Some("task") || (kind.is_none() && result.pointer("/status/state").is_some()) {
        match task_state(result) {
            SseTaskState::Completed => {
                out.push(SseEvent::Completed {
                    snapshot_text: extract_text(result),
                });
            }
            SseTaskState::Failed => {
                out.push(SseEvent::Failed {
                    reason: failure_reason(result),
                });
            }
            SseTaskState::AwaitingHuman(kind) => {
                out.push(SseEvent::AwaitingHuman {
                    kind,
                    message: awaiting_human_message(result),
                    metadata: awaiting_human_metadata(result),
                });
            }
            SseTaskState::Working | SseTaskState::Other => {}
        }
        return out;
    }
    // Legacy kind-tagged shape.
    match kind {
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
        SseTaskState::AwaitingHuman(kind) => {
            out.push(SseEvent::AwaitingHuman {
                kind,
                message: awaiting_human_message(update),
                metadata: awaiting_human_metadata(update),
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
    /// `TASK_STATE_INPUT_REQUIRED` / `TASK_STATE_AUTH_REQUIRED` — not terminal, but nothing can
    /// proceed without a human.
    AwaitingHuman(AwaitingHumanKind),
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
        "TASK_STATE_INPUT_REQUIRED" | "input-required" => {
            SseTaskState::AwaitingHuman(AwaitingHumanKind::InputRequired)
        }
        "TASK_STATE_AUTH_REQUIRED" | "auth-required" => {
            SseTaskState::AwaitingHuman(AwaitingHumanKind::AuthRequired)
        }
        _ => SseTaskState::Other,
    }
}

fn failure_reason(v: &serde_json::Value) -> String {
    v.pointer("/status/message/parts/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("task failed")
        .to_string()
}

/// Mirrors `failure_reason` — the agent's own question/prompt text, first text part only (matches
/// the reference agent's convention of one text part per pause message).
fn awaiting_human_message(v: &serde_json::Value) -> String {
    v.pointer("/status/message/parts/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("a human response is required")
        .to_string()
}

/// The agent-supplied `metadata` object on the pause (e.g. `expected_input`, `provider`,
/// `auth_url`, `hitl_request_id` — see the External Agent Contract). `Value::Null` when absent;
/// `Value::get` on `Null` returns `None` for any key, so callers can treat both cases identically.
///
/// Checks `status.message.metadata` first, then falls back to `v`'s own top-level `metadata`
/// field (`v` is either a `TaskStatusUpdateEvent` or a full `Task`, both of which carry their own
/// `metadata`) — mirrors `build_pause_question`'s identical fallback. The fallback is not a
/// theoretical case: the Python a2a-sdk's `TaskUpdater.update_status(metadata=...)` attaches
/// `metadata` to the `TaskStatusUpdateEvent` itself, not to the status message, so any agent using
/// that (standard, documented) call shape — e.g. `archive`'s `_apply_outcome` — needs this
/// fallback or its pause's `hitl_request_id` link is silently lost. Without it, a mirror row this
/// agent creates via the orchestrator (which parses through this function, not
/// `build_pause_question`) never links back to the real `mcp_tool` row, and the human sees both
/// as separate, unlinked pending requests.
fn awaiting_human_metadata(v: &serde_json::Value) -> serde_json::Value {
    v.pointer("/status/message/metadata")
        .or_else(|| v.get("metadata"))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

pub fn extract_text_from_response(response: &JsonRpcResponse) -> Option<String> {
    extract_text(response.result.as_ref()?)
}

// ─── Stream disposition (HITL) ──────────────────────────────────────────────

/// What an SSE relay loop should do with one decoded event, per the HITL plan (§8). Distinct from
/// [`SseEvent`]/[`classify_sse_event`] above, which extract *content* to relay to the client —
/// this classifies the task's *lifecycle*, so a relay loop knows whether to keep reading, stop
/// because the task is genuinely done, or stop because a human is now needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDisposition {
    /// Still working — keep relaying.
    Continue,
    /// A2A task terminal: success.
    Completed,
    /// A2A task terminal: failure, canceled, or rejected.
    Failed,
    /// A2A task NOT terminal, but this stream must stop relaying — a human is needed
    /// (`input-required`/`auth-required`).
    Paused,
}

/// Classify one A2A SSE `data:` JSON payload's stream disposition. Supersedes the old
/// `is_terminal_event` boolean check that only this crate's callers used to have — same JSON
/// navigation (JSONRPC-wrapped or bare, `statusUpdate`-nested or flat, the 0.3-dialect `final`
/// bool fallback), widened from a 2-way (terminal/not) to a 4-way outcome so a real
/// `input-required`/`auth-required` pause is distinguished from both "still working" and "truly
/// done" — today `input-required` falls through as non-terminal and the caller hangs waiting for
/// a stream close that never comes.
pub fn classify_stream_disposition(data: &str) -> StreamDisposition {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return StreamDisposition::Continue;
    };
    let result = parsed.get("result").unwrap_or(&parsed);
    // `.task` handles the non-streaming, v1.0 task-wrapped dialect (a full `Task` snapshot with
    // its own `status.state`, no `statusUpdate` event wrapper at all) — same fallback
    // `extract_text` already has, needed here too or a non-streaming pause is never classified
    // as `Paused` at all (confirmed live: this exact gap let a real agent_proxy.rs pause through
    // undetected before this fallback was added).
    let status_update = result
        .get("statusUpdate")
        .or_else(|| parsed.get("statusUpdate"))
        .or_else(|| result.get("task"))
        .unwrap_or(result);

    if let Some(state) = status_update
        .pointer("/status/state")
        .and_then(|s| s.as_str())
    {
        let state = state.to_ascii_lowercase();
        if state.contains("input_required") || state.contains("input-required") {
            return StreamDisposition::Paused;
        }
        if state.contains("auth_required") || state.contains("auth-required") {
            return StreamDisposition::Paused;
        }
        if state.contains("completed") {
            return StreamDisposition::Completed;
        }
        if state.contains("failed")
            || state.contains("canceled")
            || state.contains("cancelled")
            || state.contains("rejected")
        {
            return StreamDisposition::Failed;
        }
    }

    // 0.3 dialect: {"result": {"kind": "status-update", "final": true}} with no recognizable
    // `status.state` — a final event whose outcome couldn't be classified is treated as a
    // (successful) completion, matching the old `is_terminal_event`'s behavior.
    if result
        .get("final")
        .and_then(|f| f.as_bool())
        .unwrap_or(false)
    {
        return StreamDisposition::Completed;
    }

    StreamDisposition::Continue
}

// ─── Pause parsing (HITL) ───────────────────────────────────────────────────
//
// Shared by every consumer that needs to turn a `Paused`-classified A2A payload into a
// `hitl_requests` question: `oss/server/src/router/a2a_dispatch.rs` (direct chat, streaming and
// non-streaming) and `oss/orchestrator/src/maf/executor.rs` (MAF, a plain-JSON `SendMessage`
// reply, no SSE at all) — both parse the identical wire shapes `classify_stream_disposition`
// above already navigates. Lives here rather than in `oss/server` so `oss/orchestrator`, which
// cannot depend on `oss/server`, can reuse it too.

/// Why a `Paused`-classified payload asked for a human — the only two kinds
/// `classify_stream_disposition` ever maps to `Paused`. Deliberately not `nasiko_hitl::HitlKind`:
/// this crate must not gain a dependency on `oss/hitl`; callers that need a `HitlKind` map this
/// two-variant enum to their own type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseReason {
    InputRequired,
    AuthRequired,
}

/// Derive a pause's reason from a `Paused`-classified payload — `AuthRequired` if the wire state
/// names it, `InputRequired` otherwise.
pub fn pause_reason(data: &str) -> PauseReason {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return PauseReason::InputRequired;
    };
    let result = parsed.get("result").unwrap_or(&parsed);
    // See `classify_stream_disposition`'s doc comment for why `.task` is needed here too.
    let status_update = result
        .get("statusUpdate")
        .or_else(|| parsed.get("statusUpdate"))
        .or_else(|| result.get("task"))
        .unwrap_or(result);
    let state = status_update
        .pointer("/status/state")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if state.contains("auth_required") || state.contains("auth-required") {
        PauseReason::AuthRequired
    } else {
        PauseReason::InputRequired
    }
}

/// Extract the *agent's own* `taskId` from a `Paused` payload — never the caller's locally minted
/// `task_id`/`context_id` param used for its own outbound envelope. Real a2a-sdk agents pass their
/// real task/context ids through verbatim. Resume must target *this* id — the one the agent's own
/// task store actually holds — not Nasiko's synthetic per-request id, or the agent will never
/// recognize the follow-up as a continuation. Falls back to the caller's `task_id` only if the
/// payload carries none (e.g. a bare `message` reply with no task wrapper at all).
pub fn paused_task_id(data: &str, fallback: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return fallback.to_string();
    };
    let result = parsed.get("result").unwrap_or(&parsed);
    let status_update = result
        .get("statusUpdate")
        .or_else(|| parsed.get("statusUpdate"))
        .or_else(|| result.get("task"))
        .unwrap_or(result);

    status_update
        .get("taskId")
        // Task-wrapped dialect: the task object's own id field is `id`, not `taskId` — already
        // covered by this existing fallback, unchanged.
        .or_else(|| result.pointer("/task/id"))
        .or_else(|| {
            // Flat A2A 0.3.x Task: no `task` wrapper at all, so `result` IS the task and its own
            // id field is `id`, not `taskId` — the same shape `classify_sse_event`'s flat-Task
            // arm recognizes, gated identically (`kind == "task"`, or a kind-less payload that
            // still carries `status.state`) so the two functions can never disagree again about
            // which bytes are a flat Task. Missing this meant a flat-Task pause was correctly
            // classified as `Paused` but resumed onto the caller's synthetic fallback id instead
            // of the agent's real task, opening a brand-new task instead of continuing the
            // paused one (found in review — same defect family as the flat-Task classifier gap).
            let kind = result.get("kind").and_then(|k| k.as_str());
            let is_flat_task = kind == Some("task")
                || (kind.is_none() && result.pointer("/status/state").is_some());
            is_flat_task.then(|| result.get("id")).flatten()
        })
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| fallback.to_string())
}

/// Well-known `metadata` keys the External Agent Contract documents (`auth_url`/`provider` for
/// `auth_required`, `expected_input` for `input_required`) — hoisted onto `question` itself so a
/// client can read `question.auth_url` directly instead of reaching into an opaque `metadata`
/// blob, matching `hitl_requests.question`'s documented shape.
const WELL_KNOWN_QUESTION_KEYS: &[&str] = &["auth_url", "provider", "expected_input"];

/// Structured-options bounds (Nasiko HITL selectable-options extension, additive to
/// `input_required`). Purely defensive — nothing in the contract requires a cap, but
/// `question`/`hitl_requests.question` rides on every stream frame and session-load response
/// (`docs/FRONTEND_HITL_API_CONTRACT.md`), so an agent bug or malicious agent must not be able to
/// balloon it unboundedly.
const MAX_OPTIONS: usize = 20;
const MAX_OPTION_LABEL_LEN: usize = 200;
const MAX_OPTION_DESCRIPTION_LEN: usize = 2000;
const MAX_QUESTION_HEADER_LEN: usize = 200;

/// Build the `hitl_requests.question` JSONB from a `Paused` payload: the message text, plus
/// whatever `metadata` the agent attached. See [`pause_question`] for the hoisting rules.
pub fn build_pause_question(data: &str) -> serde_json::Value {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) else {
        return serde_json::json!({ "message": "" });
    };
    let result = parsed.get("result").unwrap_or(&parsed);
    // See `classify_stream_disposition`'s doc comment for why `.task` is needed here too.
    let status_update = result
        .get("statusUpdate")
        .or_else(|| parsed.get("statusUpdate"))
        .or_else(|| result.get("task"))
        .unwrap_or(result);

    let message = status_update
        .pointer("/status/message/parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text")?.as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let metadata = status_update
        .pointer("/status/message/metadata")
        .or_else(|| status_update.get("metadata"))
        .cloned();

    // Correlation only (log context for `hoist_structured_options`'s drop warnings) — never
    // written onto `question` itself; `paused_task_id` (a separate, more permissive function with
    // its own fallback) is what resume actually addresses.
    let task_id = status_update.get("taskId").and_then(|v| v.as_str());

    pause_question(&message, metadata, task_id)
}

/// [`build_pause_question`] for a caller that already holds the decoded pause.
///
/// The orchestrator reaches a sub-agent's pause as a typed `PauseInfo` (message + metadata),
/// never as the raw SSE payload, so it cannot call `build_pause_question` — and hand-rolling
/// `{message, metadata}` there left every hoisted key buried in `metadata`, where no client
/// looks: an orchestrator-origin selectable-options question arrived with no `question.options`
/// and rendered as a plain text box, and an `auth_required` one with no `question.auth_url` had
/// nowhere to send the human. Both origins go through this now, so `question` has one shape
/// regardless of which path minted it, and `options` is validated exactly once either way.
pub fn pause_question(
    message: &str,
    metadata: Option<serde_json::Value>,
    task_id: Option<&str>,
) -> serde_json::Value {
    let mut question = serde_json::json!({ "message": message });
    if let Some(metadata) = metadata
        && let Some(obj) = question.as_object_mut()
    {
        if let Some(metadata_obj) = metadata.as_object() {
            for key in WELL_KNOWN_QUESTION_KEYS {
                if let Some(value) = metadata_obj.get(*key) {
                    obj.insert((*key).to_string(), value.clone());
                }
            }
            hoist_structured_options(obj, metadata_obj, task_id);
        }
        obj.insert("metadata".to_string(), metadata);
    }
    question
}

/// Hoists the selectable-options extension (`header`/`options`/`multi_select`/
/// `allow_custom_input`) from the agent's `metadata` onto `question`, the same way
/// `auth_url`/`provider`/`expected_input` are hoisted — an additive extension of `input_required`,
/// not a new pause kind or wire event (§3/§19 of the request: keep `TASK_STATE_INPUT_REQUIRED`,
/// carry the richer shape through the existing metadata/question mechanism).
///
/// `options` is validated here, once, at the single choke point every origin (direct chat, the
/// orchestrator, MAF, and the resume dispatcher's own follow-up-pause construction) already shares
/// for turning a paused payload into a `question` — see this function's caller. A malformed block
/// (no `options` array, an option missing/empty/oversized `label`, or two options with the same
/// `label`) is dropped in its entirety rather than surfacing a broken or ambiguous prompt to the
/// human: the agent still gets its plain-text `input_required` pause exactly as if it had never
/// attempted structured options, instead of the whole pause failing over an agent-side bug in a
/// value the platform doesn't strictly need to render a working prompt. Labels must be unique
/// because a label is the semantic answer value (§4/§13 of the request) — an ambiguous label set
/// would make a human's selection unresolvable on resume.
///
/// Every drop path logs a `tracing::warn!` (`task_id` — the agent's own task id, best-effort, for
/// correlation — is `None` for a bare `message`-only reply with no task wrapper at all) so an
/// agent shipping a broken `options` block is observable rather than silently degrading forever —
/// the fallback itself is unchanged; only its visibility is new.
fn hoist_structured_options(
    question: &mut serde_json::Map<String, serde_json::Value>,
    metadata_obj: &serde_json::Map<String, serde_json::Value>,
    task_id: Option<&str>,
) {
    let warn_dropped = |reason: &str| {
        tracing::warn!(
            task_id = task_id.unwrap_or("unknown"),
            reason,
            "dropping malformed/ambiguous structured HITL options — falling back to a plain input_required question"
        );
    };

    if let Some(header) = metadata_obj.get("header").and_then(|v| v.as_str()) {
        let header = header.trim();
        if !header.is_empty() && header.len() <= MAX_QUESTION_HEADER_LEN {
            question.insert("header".to_string(), serde_json::json!(header));
        }
    }

    let Some(raw_options) = metadata_obj.get("options").and_then(|v| v.as_array()) else {
        return;
    };
    if raw_options.is_empty() {
        warn_dropped("options array is empty");
        return;
    }
    if raw_options.len() > MAX_OPTIONS {
        warn_dropped("options array exceeds the maximum allowed option count");
        return;
    }

    let mut labels_seen = std::collections::HashSet::with_capacity(raw_options.len());
    let mut options = Vec::with_capacity(raw_options.len());
    for raw_option in raw_options {
        let Some(label) = raw_option.get("label").and_then(|v| v.as_str()) else {
            warn_dropped("an option is missing its label");
            return;
        };
        let label = label.trim();
        if label.is_empty() || label.len() > MAX_OPTION_LABEL_LEN {
            warn_dropped("an option label is empty or exceeds the maximum allowed length");
            return;
        }
        if !labels_seen.insert(label) {
            // Duplicate label — ambiguous as a semantic answer value, drop the whole block.
            warn_dropped("two options share the same label");
            return;
        }
        let description = raw_option
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|d| !d.is_empty() && d.len() <= MAX_OPTION_DESCRIPTION_LEN);

        let mut option = serde_json::Map::with_capacity(2);
        option.insert("label".to_string(), serde_json::json!(label));
        if let Some(description) = description {
            option.insert("description".to_string(), serde_json::json!(description));
        }
        options.push(serde_json::Value::Object(option));
    }

    question.insert("options".to_string(), serde_json::Value::Array(options));
    question.insert(
        "multi_select".to_string(),
        serde_json::json!(
            metadata_obj
                .get("multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        ),
    );
    question.insert(
        "allow_custom_input".to_string(),
        serde_json::json!(
            metadata_obj
                .get("allow_custom_input")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        ),
    );
}

#[cfg(test)]
mod pause_parsing_tests {
    use super::*;

    // Real payload shape (python a2a-sdk, JSONRPC-wrapped, no "kind" tag) — confirmed live
    // against the github-hitl-agent reference agent.
    const REAL_PAUSE_PAYLOAD: &str = r#"{"result": {"statusUpdate": {"taskId": "51914422-4548-47af-90b8-773a5ee4bed7", "contextId": "9e110c60-185f-4c3d-b14e-a473db66ed4c", "status": {"state": "TASK_STATE_INPUT_REQUIRED", "message": {"messageId": "6d3e7f9d-dea0-499b-af76-a3d12eff55bf", "contextId": "9e110c60-185f-4c3d-b14e-a473db66ed4c", "taskId": "51914422-4548-47af-90b8-773a5ee4bed7", "role": "ROLE_AGENT", "parts": [{"text": "Which repository should I create the issue in? (reply with owner/repo, on the same task)"}]}, "timestamp": "2026-08-29T13:09:49.292552Z"}}}, "id": "100260a7-0f6a-4606-a411-3a72c0cfa21e", "jsonrpc": "2.0"}"#;

    #[test]
    fn paused_task_id_extracts_the_agents_real_task_not_the_callers_synthetic_one() {
        let extracted = paused_task_id(REAL_PAUSE_PAYLOAD, "nasiko-synthetic-task-id");
        assert_eq!(extracted, "51914422-4548-47af-90b8-773a5ee4bed7");
    }

    #[test]
    fn paused_task_id_falls_back_when_the_payload_carries_no_task_id() {
        let extracted =
            paused_task_id(r#"{"message": {"parts": [{"text": "hi"}]}}"#, "fallback-id");
        assert_eq!(extracted, "fallback-id");
    }

    #[test]
    fn paused_task_id_falls_back_on_unparseable_payload() {
        let extracted = paused_task_id("not json", "fallback-id");
        assert_eq!(extracted, "fallback-id");
    }

    /// Flat A2A 0.3.x Task, `kind`-tagged — no `task` wrapper, `result` IS the task, whose own id
    /// field is `id`, not `taskId`. Same fixture shape `both_classifiers_agree_on_every_pause_dialect`
    /// (`oss/types/tests/stream_disposition.rs`) uses to pin the classifiers; this pins the third
    /// function that reads the identical bytes and, before this fix, silently discarded the real id.
    #[test]
    fn paused_task_id_extracts_a_flat_tasks_own_id_field() {
        let data =
            r#"{"result": {"id": "t1", "kind": "task", "status": {"state": "input-required"}}}"#;
        assert_eq!(paused_task_id(data, "fallback-id"), "t1");
    }

    /// Same shape, no `kind` discriminator at all — the exact divergence `classify_sse_event`'s
    /// flat-Task arm was widened to close; `paused_task_id` had the identical gap one function over.
    #[test]
    fn paused_task_id_extracts_a_flat_kindless_tasks_own_id_field() {
        let data = r#"{"result": {"id": "t1", "status": {"state": "input-required"}}}"#;
        assert_eq!(paused_task_id(data, "fallback-id"), "t1");
    }

    /// A legacy kind-tagged `status-update` (no `task`/`taskId`/flat-Task `id` semantics) must
    /// keep falling back — it is not a task snapshot, so grabbing a bare top-level `id` here would
    /// be wrong even if one happened to be present.
    #[test]
    fn paused_task_id_does_not_treat_a_legacy_status_update_as_a_flat_task() {
        let data = r#"{"result": {"id": "not-a-task-id", "kind": "status-update",
                          "status": {"state": "working"}}}"#;
        assert_eq!(paused_task_id(data, "fallback-id"), "fallback-id");
    }

    #[test]
    fn pause_reason_reads_input_required_from_the_real_payload() {
        assert_eq!(pause_reason(REAL_PAUSE_PAYLOAD), PauseReason::InputRequired);
    }

    #[test]
    fn build_pause_question_extracts_the_message_text_from_the_real_payload() {
        let question = build_pause_question(REAL_PAUSE_PAYLOAD);
        assert_eq!(
            question["message"],
            "Which repository should I create the issue in? (reply with owner/repo, on the same task)"
        );
    }

    const AUTH_REQUIRED_PAYLOAD: &str = r#"{"result": {"statusUpdate": {"taskId": "t1", "contextId": "c1", "status": {"state": "TASK_STATE_AUTH_REQUIRED", "message": {"parts": [{"text": "Please authorize with GitHub"}], "metadata": {"provider": "github", "auth_url": "https://github.com/login/oauth/authorize?client_id=abc"}}}}}, "id": "1", "jsonrpc": "2.0"}"#;

    #[test]
    fn build_pause_question_hoists_auth_url_and_provider_to_the_top_level() {
        let question = build_pause_question(AUTH_REQUIRED_PAYLOAD);
        assert_eq!(question["message"], "Please authorize with GitHub");
        assert_eq!(question["provider"], "github");
        assert_eq!(
            question["auth_url"],
            "https://github.com/login/oauth/authorize?client_id=abc"
        );
        // The full metadata blob is still kept underneath, unmodified.
        assert_eq!(question["metadata"]["provider"], "github");
    }

    /// The orchestrator holds a decoded `PauseInfo`, not the raw payload, so it persists its
    /// pause through `pause_question` instead. It must land on the identical shape — it did not,
    /// and every orchestrator-origin options question rendered as a plain text box because
    /// `question.options` was buried one level down in `metadata`.
    #[test]
    fn pause_question_hoists_the_same_keys_as_the_raw_payload_path() {
        let metadata = serde_json::json!({
            "auth_url": "https://github.com/login/oauth/authorize?client_id=abc",
            "provider": "github",
            "options": [{"label": "Summary"}, {"label": "Detailed"}],
            "multi_select": true,
            "allow_custom_input": true,
        });
        let question = pause_question("Pick the sections", Some(metadata), Some("t1"));

        assert_eq!(question["message"], "Pick the sections");
        assert_eq!(question["provider"], "github");
        assert_eq!(
            question["auth_url"],
            "https://github.com/login/oauth/authorize?client_id=abc"
        );
        assert_eq!(question["options"][1]["label"], "Detailed");
        assert_eq!(question["multi_select"], true);
        assert_eq!(question["allow_custom_input"], true);
        // And the raw-payload path is this function plus parsing, not a second implementation.
        assert_eq!(
            build_pause_question(AUTH_REQUIRED_PAYLOAD),
            pause_question(
                "Please authorize with GitHub",
                Some(serde_json::json!({
                    "provider": "github",
                    "auth_url": "https://github.com/login/oauth/authorize?client_id=abc"
                })),
                Some("t1"),
            )
        );
    }

    #[test]
    fn pause_reason_reads_auth_required_from_the_real_payload() {
        assert_eq!(
            pause_reason(AUTH_REQUIRED_PAYLOAD),
            PauseReason::AuthRequired
        );
    }

    // ── Structured options extension (selectable-options HITL) ─────────────────────────────

    fn payload_with_metadata(metadata: serde_json::Value) -> String {
        serde_json::json!({
            "result": {
                "statusUpdate": {
                    "taskId": "t1",
                    "contextId": "c1",
                    "status": {
                        "state": "TASK_STATE_INPUT_REQUIRED",
                        "message": {
                            "parts": [{"text": "How should I format the output?"}],
                            "metadata": metadata,
                        }
                    }
                }
            },
            "id": "1",
            "jsonrpc": "2.0",
        })
        .to_string()
    }

    #[test]
    fn build_pause_question_hoists_well_formed_options() {
        let payload = payload_with_metadata(serde_json::json!({
            "header": "Format",
            "options": [
                {"label": "Summary", "description": "Brief overview"},
                {"label": "Detailed", "description": "Full explanation"},
            ],
            "multi_select": false,
            "allow_custom_input": true,
        }));
        let question = build_pause_question(&payload);
        assert_eq!(question["header"], "Format");
        assert_eq!(question["multi_select"], false);
        assert_eq!(question["allow_custom_input"], true);
        let options = question["options"].as_array().unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0]["label"], "Summary");
        assert_eq!(options[0]["description"], "Brief overview");
        assert_eq!(options[1]["label"], "Detailed");
    }

    #[test]
    fn build_pause_question_defaults_multi_select_and_allow_custom_input_to_false() {
        let payload = payload_with_metadata(serde_json::json!({
            "options": [{"label": "Yes"}, {"label": "No"}],
        }));
        let question = build_pause_question(&payload);
        assert_eq!(question["multi_select"], false);
        assert_eq!(question["allow_custom_input"], false);
        // description is genuinely optional per option.
        assert!(question["options"][0].get("description").is_none());
    }

    #[test]
    fn build_pause_question_drops_options_with_duplicate_labels() {
        let payload = payload_with_metadata(serde_json::json!({
            "options": [{"label": "Summary"}, {"label": "Summary"}],
        }));
        let question = build_pause_question(&payload);
        assert!(
            question.get("options").is_none(),
            "duplicate labels are ambiguous as answer values and must drop the whole block"
        );
        assert!(question.get("multi_select").is_none());
        assert!(question.get("allow_custom_input").is_none());
        // The plain-text pause must still work — this is a fallback, not a failure.
        assert_eq!(question["message"], "How should I format the output?");
    }

    #[test]
    fn build_pause_question_drops_options_with_an_empty_label() {
        let payload = payload_with_metadata(serde_json::json!({
            "options": [{"label": ""}, {"label": "No"}],
        }));
        let question = build_pause_question(&payload);
        assert!(question.get("options").is_none());
    }

    #[test]
    fn build_pause_question_drops_options_missing_a_label() {
        let payload = payload_with_metadata(serde_json::json!({
            "options": [{"description": "no label here"}],
        }));
        let question = build_pause_question(&payload);
        assert!(question.get("options").is_none());
    }

    #[test]
    fn build_pause_question_drops_an_empty_options_array() {
        let payload = payload_with_metadata(serde_json::json!({ "options": [] }));
        let question = build_pause_question(&payload);
        assert!(question.get("options").is_none());
    }

    #[test]
    fn build_pause_question_drops_options_beyond_the_cap() {
        let too_many: Vec<_> = (0..(MAX_OPTIONS + 1))
            .map(|i| serde_json::json!({"label": format!("option-{i}")}))
            .collect();
        let payload = payload_with_metadata(serde_json::json!({ "options": too_many }));
        let question = build_pause_question(&payload);
        assert!(question.get("options").is_none());
    }

    #[test]
    fn build_pause_question_hoists_header_independently_of_options() {
        let payload = payload_with_metadata(serde_json::json!({ "header": "Format" }));
        let question = build_pause_question(&payload);
        assert_eq!(question["header"], "Format");
        assert!(question.get("options").is_none());
    }

    #[test]
    fn build_pause_question_with_no_options_key_behaves_exactly_as_before() {
        // The pre-existing, non-structured input_required shape — must be completely unaffected.
        let payload = payload_with_metadata(serde_json::json!({}));
        let question = build_pause_question(&payload);
        assert!(question.get("options").is_none());
        assert!(question.get("multi_select").is_none());
        assert!(question.get("allow_custom_input").is_none());
        assert!(question.get("header").is_none());
    }

    // Captured live from `agent_proxy.rs`'s non-streaming branch (`SendMessage`, not
    // `SendStreamingMessage`) against `github-hitl-agent` — a full `Task` snapshot with no
    // `statusUpdate` wrapper at all, `status.state` sitting under `result.task` instead.
    const REAL_NON_STREAMING_TASK_SNAPSHOT_PAYLOAD: &str = r#"{"result":{"task":{"id":"3f43589f-fbaf-4936-8cac-e074b5843302","contextId":"nonstream-proxy-test","status":{"state":"TASK_STATE_INPUT_REQUIRED","message":{"messageId":"6c334d6d-e72b-4e3d-8e25-ac1a223085b9","contextId":"nonstream-proxy-test","taskId":"3f43589f-fbaf-4936-8cac-e074b5843302","role":"ROLE_AGENT","parts":[{"text":"Which repository should I create the issue in? (reply with owner/repo, on the same task)"}]},"timestamp":"2026-09-01T03:42:34.957766Z"},"history":[{"messageId":"293C90CE-26DB-4364-895D-A4A059DDC75E","contextId":"nonstream-proxy-test","taskId":"3f43589f-fbaf-4936-8cac-e074b5843302","role":"ROLE_USER","parts":[{"text":"hitl input test"}]}]}},"id":"1","jsonrpc":"2.0"}"#;

    #[test]
    fn pause_reason_reads_input_required_from_a_task_wrapped_non_streaming_payload() {
        assert_eq!(
            pause_reason(REAL_NON_STREAMING_TASK_SNAPSHOT_PAYLOAD),
            PauseReason::InputRequired
        );
    }

    #[test]
    fn paused_task_id_extracts_the_real_id_from_a_task_wrapped_non_streaming_payload() {
        assert_eq!(
            paused_task_id(REAL_NON_STREAMING_TASK_SNAPSHOT_PAYLOAD, "fallback"),
            "3f43589f-fbaf-4936-8cac-e074b5843302"
        );
    }

    #[test]
    fn build_pause_question_reads_the_message_from_a_task_wrapped_non_streaming_payload() {
        let question = build_pause_question(REAL_NON_STREAMING_TASK_SNAPSHOT_PAYLOAD);
        assert_eq!(
            question["message"],
            "Which repository should I create the issue in? (reply with owner/repo, on the same task)"
        );
    }
}

// ─── Private ────────────────────────────────────────────────────────────────

fn build_request(
    method: &str,
    text: &str,
    context_id: Option<&str>,
    extra_parts: &[Part],
    task_id: Option<&str>,
) -> JsonRpcRequest {
    let ctx = context_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let mut parts = vec![text_part(text)];
    parts.extend(extra_parts.iter().cloned());

    let message = Message {
        message_id: uuid::Uuid::new_v4().to_string(),
        context_id: Some(ctx),
        task_id: task_id.map(String::from),
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

    // InputRequired/AuthRequired must classify to AwaitingHuman, never StatusText — the whole
    // reason this variant exists is so a caller can't mistake the agent's question for ordinary
    // progress narration.

    #[test]
    fn input_required_status_classifies_as_awaiting_human() {
        let ev = json!({"result": {"statusUpdate": {"status": {
            "state": "TASK_STATE_INPUT_REQUIRED",
            "message": {
                "parts": [{"text": "Which repository?"}],
                "metadata": {"expected_input": "free_text"}
            }
        }}}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::InputRequired,
                message: "Which repository?".into(),
                metadata: json!({"expected_input": "free_text"}),
            }]
        );
    }

    #[test]
    fn auth_required_status_classifies_as_awaiting_human_not_status_text() {
        let ev = json!({"result": {"statusUpdate": {"status": {
            "state": "TASK_STATE_AUTH_REQUIRED",
            "message": {
                "parts": [{"text": "Authorize GitHub access"}],
                "metadata": {"provider": "github", "auth_url": "https://github.com/login/oauth"}
            }
        }}}});
        let events = classify_sse_event(&ev);
        assert_eq!(
            events,
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::AuthRequired,
                message: "Authorize GitHub access".into(),
                metadata: json!({"provider": "github", "auth_url": "https://github.com/login/oauth"}),
            }]
        );
        assert!(
            !events.iter().any(|e| matches!(e, SseEvent::StatusText(_))),
            "an auth-required pause must never also/instead classify as StatusText"
        );
    }

    // Regression: the flat A2A 0.3.x Task shape (`a2a-lf` 0.3.0, python/langgraph SDKs) has no
    // `task` wrapper at all — the task IS `result`. This previously produced zero events, so a
    // sub-agent's pause was silently handed back to the orchestrating LLM as a successful result.
    #[test]
    fn flat_task_shape_input_required_classifies_as_awaiting_human() {
        let ev = json!({"result": {
            "id": "t1", "contextId": "c1", "kind": "task",
            "status": {"state": "input-required",
                       "message": {"role": "agent", "parts": [{"text": "Which repo?"}]}}
        }});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::InputRequired,
                message: "Which repo?".into(),
                metadata: serde_json::Value::Null,
            }]
        );
    }

    /// Same flat Task, minus the `kind` discriminator the 0.3 spec calls for. Gating the arm on
    /// `kind` alone left this returning `[]` while `classify_stream_disposition` called the very
    /// same bytes `Paused` (found in review) — a sub-agent's pause handed back to the
    /// orchestrating LLM as a successful tool result.
    #[test]
    fn flat_task_without_a_kind_discriminator_still_classifies_as_awaiting_human() {
        let ev = json!({"result": {
            "id": "t1", "contextId": "c1",
            "status": {"state": "input-required",
                       "message": {"role": "agent", "parts": [{"text": "Which repo?"}]}}
        }});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::InputRequired,
                message: "Which repo?".into(),
                metadata: serde_json::Value::Null,
            }]
        );
    }

    /// The guard on the arm above: a legacy kind-tagged `status-update` also carries
    /// `status.state`, so a gate of "anything with a state is a task" would capture it here and
    /// drop the `StatusData` parts only `classify_status` extracts.
    #[test]
    fn a_working_status_update_is_not_captured_by_the_flat_task_arm() {
        let ev = json!({"result": {"kind": "status-update", "status": {
            "state": "working",
            "message": {"parts": [{"data": {"type": "tool_call"}}]}
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::StatusData(json!({"type": "tool_call"}))]
        );
    }

    #[test]
    fn flat_task_shape_completed_carries_snapshot_text() {
        let ev = json!({"result": {
            "id": "t1", "contextId": "c1", "kind": "task",
            "status": {"state": "completed",
                       "message": {"role": "agent", "parts": [{"text": "done"}]}}
        }});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::Completed {
                snapshot_text: Some("done".into())
            }]
        );
    }

    // Regression: the Python a2a-sdk's `TaskUpdater.update_status(metadata=...)` attaches
    // `metadata` to the `TaskStatusUpdateEvent` itself (a sibling of `status`), not to
    // `status.message.metadata` — a real, live shape (`archive`'s `_apply_outcome`), not a
    // hypothetical one. Losing this metadata silently drops the pause's `hitl_request_id`
    // mirror link, so the orchestrator's own discovery surface shows two unlinked pending rows
    // (the real `mcp_tool` row and an orphaned mirror) for what is really one event.
    #[test]
    fn auth_required_status_reads_metadata_from_the_sibling_status_update_field_too() {
        let ev = json!({"result": {"statusUpdate": {
            "metadata": {"auth_kind": "mcp_tool_approval", "hitl_request_id": "20c0fe2b-beb3-4b85-8430-030d7c964d1c"},
            "status": {
                "state": "TASK_STATE_AUTH_REQUIRED",
                "message": {"parts": [{"text": "Tool(s) require user approval for this agent."}]}
            }
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::AuthRequired,
                message: "Tool(s) require user approval for this agent.".into(),
                metadata: json!({
                    "auth_kind": "mcp_tool_approval",
                    "hitl_request_id": "20c0fe2b-beb3-4b85-8430-030d7c964d1c",
                }),
            }]
        );
    }

    #[test]
    fn input_required_task_snapshot_classifies_as_awaiting_human() {
        // The "full task snapshot" wire shape (a2a-go-style single-object reply), not just the
        // streamed statusUpdate shape — both must recognize the pause.
        let ev = json!({"result": {"task": {
            "status": {
                "state": "TASK_STATE_INPUT_REQUIRED",
                "message": {"parts": [{"text": "Which repository?"}]}
            }
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::InputRequired,
                message: "Which repository?".into(),
                metadata: serde_json::Value::Null,
            }]
        );
    }

    #[test]
    fn legacy_lowercase_input_required_state_classifies_as_awaiting_human() {
        let ev = json!({"statusUpdate": {"status": {
            "state": "input-required",
            "message": {"parts": [{"text": "Which repository?"}]}
        }}});
        assert_eq!(
            classify_sse_event(&ev),
            vec![SseEvent::AwaitingHuman {
                kind: AwaitingHumanKind::InputRequired,
                message: "Which repository?".into(),
                metadata: serde_json::Value::Null,
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
