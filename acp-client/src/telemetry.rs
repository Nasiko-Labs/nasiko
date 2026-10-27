//! Build the shared [`CodingAgentEventV1`] from one ACP turn's streamed
//! `session/update`s.
//!
//! ACP updates carry the agent's response text and its tool calls, but **not**
//! the model name or per-call token counts — the only usage ACP exposes is an
//! optional, feature-gated, model-less, turn-aggregate figure on the prompt
//! response. Token/cost accounting for ACP coding agents comes from the LLM
//! router instead, correlated by the shared trace. So this event leaves
//! [`turn.llm_calls`](CodingAgentTurn::llm_calls) empty and records the response
//! plus the tool calls; tool calls are associated at turn granularity (no
//! `model_call_id`, since there is no LLM call to point at).
//!
//! Capture policy is enforced here, before the event is handed on: a
//! metadata-only event carries no prompt/response/title and no tool
//! argument/output content, mirroring the CLI's `canonical_event`.
//!
//! A failed or interrupted turn still produces a best-effort event (NAS-841 /
//! TRD §6): whatever was gathered is emitted, incomplete tool calls keep their
//! last status, and the deterministic `event_id` makes a later full re-emit of
//! the same turn dedup cleanly.

use std::collections::HashMap;

use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, ToolCall, ToolCallStatus, ToolCallUpdate, ToolKind,
};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CODING_AGENT_CONTENT_MAX_BYTES, CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1,
    CodingAgentSession, CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
    CodingAgentToolCall, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id,
    coding_agent_session_id,
};

/// Failure building a telemetry event.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The assembled event failed `CodingAgentEventV1::validate`.
    #[error("built telemetry event is invalid: {0}")]
    Invalid(String),
}

/// Accumulates one turn's `session/update`s into a [`CodingAgentEventV1`].
///
/// Seed it when the turn starts, feed it each update with [`observe`], then
/// [`finish`] it to get the event.
///
/// [`observe`]: TurnTelemetry::observe
/// [`finish`]: TurnTelemetry::finish
#[derive(Debug, Clone)]
pub struct TurnTelemetry {
    agent_id: String,
    agent_name: String,
    source_session_id: String,
    turn_id: String,
    prompt: String,
    capture_policy: CapturePolicy,
    started_at: DateTime<Utc>,
    response: String,
    tools: Vec<ToolAcc>,
    tool_index: HashMap<String, usize>,
}

/// One tool call, folded across its `ToolCall` + `ToolCallUpdate`s.
#[derive(Debug, Clone)]
struct ToolAcc {
    id: String,
    name: String,
    kind: String,
    status: CodingAgentToolCallStatus,
    arguments: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

impl TurnTelemetry {
    /// Start accumulating a turn. `agent_id` is the coding-agent integration id
    /// (e.g. `"opencode"`), `source_session_id` the agent's own session id, and
    /// `turn_id` the per-turn id; together they derive the event's stable ids.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
        source_session_id: impl Into<String>,
        turn_id: impl Into<String>,
        prompt: impl Into<String>,
        capture_policy: CapturePolicy,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            source_session_id: source_session_id.into(),
            turn_id: turn_id.into(),
            prompt: prompt.into(),
            capture_policy,
            started_at,
            response: String::new(),
            tools: Vec::new(),
            tool_index: HashMap::new(),
        }
    }

    /// Fold one streamed update into the turn. `at` is the observation time —
    /// ACP updates carry no timestamps, so the caller passes `Utc::now()` and
    /// tool timings are inferred from it.
    pub fn observe(&mut self, update: &SessionUpdate, at: DateTime<Utc>) {
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(text) = &chunk.content {
                    self.response.push_str(&text.text);
                }
            }
            SessionUpdate::ToolCall(tool_call) => self.observe_tool_call(tool_call, at),
            SessionUpdate::ToolCallUpdate(update) => self.observe_tool_update(update, at),
            // Thoughts, plans, usage, mode/config/info, notices, compaction: not
            // part of the response text or the tool record.
            _ => {}
        }
    }

    fn observe_tool_call(&mut self, tc: &ToolCall, at: DateTime<Utc>) {
        let id = tc.tool_call_id.0.to_string();
        let name = tc.name.clone().unwrap_or_else(|| tc.title.clone());
        let status = map_status(tc.status);
        let ended = is_terminal(status).then_some(at);
        match self.tool_index.get(&id).copied() {
            Some(idx) => {
                let t = &mut self.tools[idx];
                t.name = name;
                t.kind = kind_str(tc.kind).to_owned();
                t.status = status;
                if tc.raw_input.is_some() {
                    t.arguments = tc.raw_input.clone();
                }
                if tc.raw_output.is_some() {
                    t.output = tc.raw_output.clone();
                }
                if ended.is_some() && t.ended_at.is_none() {
                    t.ended_at = ended;
                }
            }
            None => {
                self.tool_index.insert(id.clone(), self.tools.len());
                self.tools.push(ToolAcc {
                    id,
                    name,
                    kind: kind_str(tc.kind).to_owned(),
                    status,
                    arguments: tc.raw_input.clone(),
                    output: tc.raw_output.clone(),
                    started_at: at,
                    ended_at: ended,
                });
            }
        }
    }

    fn observe_tool_update(&mut self, tcu: &ToolCallUpdate, at: DateTime<Utc>) {
        let id = tcu.tool_call_id.0.to_string();
        let f = &tcu.fields;
        let idx = match self.tool_index.get(&id).copied() {
            Some(idx) => idx,
            // An update for a tool we never saw announced: seed a minimal entry
            // so the record isn't lost.
            None => {
                let name = f
                    .name
                    .clone()
                    .or_else(|| f.title.clone())
                    .unwrap_or_else(|| id.clone());
                self.tool_index.insert(id.clone(), self.tools.len());
                self.tools.push(ToolAcc {
                    id,
                    name,
                    kind: f.kind.map(kind_str).unwrap_or("other").to_owned(),
                    status: CodingAgentToolCallStatus::Unknown,
                    arguments: f.raw_input.clone(),
                    output: f.raw_output.clone(),
                    started_at: at,
                    ended_at: None,
                });
                self.tools.len() - 1
            }
        };
        let t = &mut self.tools[idx];
        if let Some(kind) = f.kind {
            t.kind = kind_str(kind).to_owned();
        }
        if let Some(name) = &f.name {
            t.name = name.clone();
        }
        if let Some(status) = f.status {
            t.status = map_status(status);
            if is_terminal(t.status) && t.ended_at.is_none() {
                t.ended_at = Some(at);
            }
        }
        if f.raw_input.is_some() {
            t.arguments = f.raw_input.clone();
        }
        if f.raw_output.is_some() {
            t.output = f.raw_output.clone();
        }
    }

    /// Assemble and validate the event. `ended_at` is the turn end (also the
    /// event's `captured_at`).
    ///
    /// Capture policy: a `Content` turn is only emitted as content when it
    /// actually has a response, so a partial/empty turn degrades to
    /// metadata-only rather than failing validation (which requires a non-empty
    /// response under `Content`).
    pub fn finish(self, ended_at: DateTime<Utc>) -> Result<CodingAgentEventV1, TelemetryError> {
        let content =
            self.capture_policy == CapturePolicy::Content && !self.response.trim().is_empty();
        let effective_policy = if content {
            CapturePolicy::Content
        } else {
            CapturePolicy::MetadataOnly
        };

        let tool_calls = self
            .tools
            .iter()
            .map(|t| CodingAgentToolCall {
                id: t.id.clone(),
                name: t.name.clone(),
                kind: t.kind.clone(),
                // No LLM call to attribute to (llm_calls is empty), so tools are
                // associated at turn granularity and carry no model_call_id.
                model_call_id: None,
                status: t.status,
                arguments: content
                    .then(|| t.arguments.clone())
                    .flatten()
                    .map(bounded_value),
                output: content
                    .then(|| t.output.clone())
                    .flatten()
                    .map(bounded_value),
                raw: None,
                error: None,
                started_at: Some(t.started_at),
                ended_at: t.ended_at,
                duration_ms: duration_ms(t.started_at, t.ended_at),
                association: CodingAgentToolAssociation::Turn,
                // Timestamps are inferred from observation time, not reported.
                timestamp_quality: CodingAgentTimestampQuality::Inferred,
            })
            .collect();

        let turn = CodingAgentTurn {
            id: self.turn_id.clone(),
            prompt: content.then(|| bounded_text(&self.prompt)),
            response: content.then(|| bounded_text(&self.response)),
            started_at: self.started_at,
            ended_at,
            // Token/cost for ACP coding agents is recorded by the LLM router,
            // correlated by the shared trace — never populated from ACP updates.
            llm_calls: Vec::new(),
            tool_calls,
        };

        let event = CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id(&self.agent_id, &self.source_session_id, &self.turn_id),
            captured_at: ended_at,
            source: CodingAgentSource {
                agent_id: self.agent_id.clone(),
                agent_name: self.agent_name.clone(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id(&self.agent_id, &self.source_session_id),
                source_id: self.source_session_id.clone(),
                title: None,
                parent: None,
            },
            turn,
            capture_policy: effective_policy,
        };

        event
            .validate()
            .map_err(TelemetryError::Invalid)
            .map(|()| event)
    }
}

fn map_status(status: ToolCallStatus) -> CodingAgentToolCallStatus {
    match status {
        ToolCallStatus::Pending => CodingAgentToolCallStatus::Pending,
        ToolCallStatus::InProgress => CodingAgentToolCallStatus::Running,
        ToolCallStatus::Completed => CodingAgentToolCallStatus::Succeeded,
        ToolCallStatus::Failed => CodingAgentToolCallStatus::Failed,
        // `ToolCallStatus` is #[non_exhaustive]; unknown future states map to Unknown.
        _ => CodingAgentToolCallStatus::Unknown,
    }
}

fn is_terminal(status: CodingAgentToolCallStatus) -> bool {
    matches!(
        status,
        CodingAgentToolCallStatus::Succeeded
            | CodingAgentToolCallStatus::Failed
            | CodingAgentToolCallStatus::Denied
            | CodingAgentToolCallStatus::TimedOut
            | CodingAgentToolCallStatus::Cancelled
    )
}

fn kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Think => "think",
        ToolKind::Fetch => "fetch",
        ToolKind::SwitchMode => "switch_mode",
        // `ToolKind` is #[non_exhaustive]; `Other` and any future kind fall here.
        _ => "other",
    }
}

fn duration_ms(started_at: DateTime<Utc>, ended_at: Option<DateTime<Utc>>) -> Option<u64> {
    let end = ended_at?;
    (end - started_at).num_milliseconds().try_into().ok()
}

/// Truncate text to the event's content cap on a UTF-8 boundary, leaving room
/// for an ellipsis so the result never exceeds the cap.
fn bounded_text(s: &str) -> String {
    const MAX: usize = CODING_AGENT_CONTENT_MAX_BYTES;
    if s.len() <= MAX {
        return s.to_owned();
    }
    let mut end = MAX.saturating_sub(3).min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Keep a JSON value as-is if it serializes within the content cap, otherwise
/// replace it with a truncated string marker (mirrors the CLI's `bounded_value`).
fn bounded_value(value: serde_json::Value) -> serde_json::Value {
    match serde_json::to_string(&value) {
        Ok(s) if s.len() <= CODING_AGENT_CONTENT_MAX_BYTES => value,
        Ok(s) => serde_json::Value::String(bounded_text(&truncate_half(&s))),
        Err(_) => serde_json::Value::Null,
    }
}

fn truncate_half(s: &str) -> String {
    let mut end = (CODING_AGENT_CONTENT_MAX_BYTES / 2).min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::{ContentChunk, TextContent, ToolCallUpdateFields};

    fn text_chunk(text: &str) -> SessionUpdate {
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        ))))
    }

    fn seed() -> TurnTelemetry {
        TurnTelemetry::new(
            "opencode",
            "my-coder",
            "sess-1",
            "turn-1",
            "do the thing",
            CapturePolicy::Content,
            Utc::now(),
        )
    }

    #[test]
    fn content_turn_captures_response_and_tool() {
        let t0 = Utc::now();
        let mut turn = seed();
        turn.observe(&text_chunk("the answer is 4"), t0);
        turn.observe(
            &SessionUpdate::ToolCall(
                ToolCall::new("t1", "read file")
                    .kind(ToolKind::Read)
                    .status(ToolCallStatus::Pending)
                    .raw_input(serde_json::json!({"path": "a.rs"})),
            ),
            t0,
        );
        turn.observe(
            &SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "t1",
                ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .raw_output(serde_json::json!({"ok": true})),
            )),
            t0 + chrono::Duration::seconds(1),
        );

        let event = turn
            .finish(t0 + chrono::Duration::seconds(2))
            .expect("valid");

        assert_eq!(event.capture_policy, CapturePolicy::Content);
        assert_eq!(event.turn.response.as_deref(), Some("the answer is 4"));
        assert_eq!(event.turn.prompt.as_deref(), Some("do the thing"));
        assert!(
            event.turn.llm_calls.is_empty(),
            "token/cost comes from the router"
        );
        assert_eq!(event.turn.tool_calls.len(), 1);

        let tool = &event.turn.tool_calls[0];
        assert_eq!(tool.id, "t1");
        assert_eq!(tool.name, "read file");
        assert_eq!(tool.kind, "read");
        assert_eq!(tool.status, CodingAgentToolCallStatus::Succeeded);
        assert_eq!(tool.model_call_id, None);
        assert_eq!(tool.association, CodingAgentToolAssociation::Turn);
        assert_eq!(
            tool.timestamp_quality,
            CodingAgentTimestampQuality::Inferred
        );
        assert!(tool.arguments.is_some());
        assert!(tool.output.is_some());
        assert!(tool.ended_at.is_some());
    }

    #[test]
    fn metadata_policy_strips_content_keeps_metadata() {
        let t0 = Utc::now();
        let mut turn = TurnTelemetry::new(
            "opencode",
            "my-coder",
            "sess-1",
            "turn-1",
            "prompt",
            CapturePolicy::MetadataOnly,
            t0,
        );
        turn.observe(&text_chunk("hi"), t0);
        turn.observe(
            &SessionUpdate::ToolCall(
                ToolCall::new("t1", "run")
                    .kind(ToolKind::Execute)
                    .status(ToolCallStatus::Completed)
                    .raw_input(serde_json::json!({"cmd": "ls"})),
            ),
            t0,
        );

        let event = turn
            .finish(t0 + chrono::Duration::seconds(1))
            .expect("valid");

        assert_eq!(event.capture_policy, CapturePolicy::MetadataOnly);
        assert_eq!(event.turn.prompt, None);
        assert_eq!(event.turn.response, None);
        let tool = &event.turn.tool_calls[0];
        assert_eq!(tool.kind, "execute");
        assert_eq!(tool.status, CodingAgentToolCallStatus::Succeeded);
        assert_eq!(tool.arguments, None, "content stripped under metadata-only");
    }

    #[test]
    fn empty_response_degrades_content_to_metadata() {
        let t0 = Utc::now();
        let mut turn = seed(); // Content policy, but we never emit a response
        turn.observe(
            &SessionUpdate::ToolCall(ToolCall::new("t1", "run").status(ToolCallStatus::Failed)),
            t0,
        );

        let event = turn
            .finish(t0 + chrono::Duration::seconds(1))
            .expect("valid");

        // No response under Content would fail validation, so it degrades.
        assert_eq!(event.capture_policy, CapturePolicy::MetadataOnly);
        assert_eq!(event.turn.response, None);
        assert_eq!(
            event.turn.tool_calls[0].status,
            CodingAgentToolCallStatus::Failed
        );
    }

    #[test]
    fn ids_are_deterministic() {
        let event = seed().finish(Utc::now()).expect("valid");
        assert_eq!(
            event.event_id,
            coding_agent_event_id("opencode", "sess-1", "turn-1")
        );
        assert_eq!(
            event.session.id,
            coding_agent_session_id("opencode", "sess-1")
        );
        assert_eq!(event.source.agent_id, "opencode");
        assert_eq!(event.session.source_id, "sess-1");
    }
}
