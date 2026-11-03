//! Conversation turns for agents the router is the only record of.
//!
//! An A2A agent the CLI discovered on a user's machine calls the router directly: nothing else
//! sees its conversation, so the router hands each of its calls to a [`TurnRecorder`] as one
//! turn (the latest user message and the reply). The host decides what a turn becomes; the
//! server keeps them as a chat session, so the agent shows up in Sessions.
//!
//! Recording must never cost the routed request anything: [`TurnRecorder::record`] is called
//! on the request path after the reply is known and must return at once, doing its work on its
//! own task, as usage logging does.

use chrono::{DateTime, Utc};

use crate::ir::{ChatResponse, ToolCallDelta, Usage};

/// One routed call, as a turn of the calling agent's conversation.
#[derive(Debug, Clone)]
pub struct RecordedTurn {
    pub agent_id: String,
    pub owner_id: String,
    /// The latest user message of the request; empty when it had none.
    pub user_text: String,
    /// The reply's text, or the tools it called when it only called tools.
    pub assistant_text: String,
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub latency_ms: i64,
    pub started_at: DateTime<Utc>,
    /// The request's span: the trace its `gen_ai.chat` span belongs to, so the host can tie
    /// the turn to that trace (the router itself stays free of OpenTelemetry types).
    pub span: tracing::Span,
}

/// Where turns go. Implementations return immediately and do their work on their own task.
pub trait TurnRecorder: Send + Sync {
    fn record(&self, turn: RecordedTurn);
}

/// Records nothing: the default, and what the router does without a host that keeps turns.
pub struct NoTurnRecorder;

impl TurnRecorder for NoTurnRecorder {
    fn record(&self, _turn: RecordedTurn) {}
}

/// The user side of a turn whose request answers tool results rather than asking anything new.
pub const TOOL_RESULTS: &str = "[tool results]";

/// What is known of a turn when its request arrives.
#[derive(Debug, Clone)]
pub(crate) struct TurnStart {
    pub user_text: String,
    pub started_at: DateTime<Utc>,
    /// Taken when the request arrives: a streamed turn finishes after the request's span.
    pub span: tracing::Span,
}

impl TurnStart {
    pub(crate) fn finish(
        self,
        ids: (String, String),
        reply: String,
        model: String,
        usage: Option<&Usage>,
        latency_ms: i64,
    ) -> RecordedTurn {
        let (agent_id, owner_id) = ids;
        RecordedTurn {
            agent_id,
            owner_id,
            user_text: self.user_text,
            assistant_text: reply,
            model,
            input_tokens: usage.and_then(|u| u.prompt_tokens),
            output_tokens: usage.and_then(|u| u.completion_tokens),
            latency_ms,
            started_at: self.started_at,
            span: self.span,
        }
    }
}

/// A non-streamed reply as turn text: its text, else the tools it called.
pub(crate) fn reply_text(response: &ChatResponse) -> String {
    let Some(message) = response.choices.first().map(|choice| &choice.message) else {
        return String::new();
    };
    match message.text() {
        Some(text) if !text.is_empty() => text,
        _ => tools_called(
            message
                .tool_calls
                .iter()
                .flatten()
                .map(|call| call.function.name.as_str()),
        ),
    }
}

/// A streamed reply, gathered chunk by chunk.
#[derive(Debug, Default)]
pub(crate) struct StreamedReply {
    text: String,
    tools: Vec<String>,
}

impl StreamedReply {
    pub(crate) fn push(&mut self, content: Option<&str>, tool_calls: Option<&[ToolCallDelta]>) {
        if let Some(content) = content {
            self.text.push_str(content);
        }
        for call in tool_calls.into_iter().flatten() {
            if let Some(name) = call.function.as_ref().and_then(|f| f.name.as_deref()) {
                self.tools.push(name.to_string());
            }
        }
    }

    pub(crate) fn text(&self) -> String {
        if self.text.is_empty() {
            tools_called(self.tools.iter().map(String::as_str))
        } else {
            self.text.clone()
        }
    }
}

fn tools_called<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names: Vec<&str> = names.collect();
    if names.is_empty() {
        String::new()
    } else {
        format!("[called {}]", names.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::FunctionCallDelta;

    fn response(message: serde_json::Value) -> ChatResponse {
        serde_json::from_value(serde_json::json!({
            "id": "r", "model": "m",
            "choices": [{"index": 0, "message": message}]
        }))
        .unwrap()
    }

    #[test]
    fn a_reply_is_its_text_or_the_tools_it_called() {
        assert_eq!(
            reply_text(&response(
                serde_json::json!({"role": "assistant", "content": "hi"})
            )),
            "hi"
        );
        let tools_only = response(serde_json::json!({
            "role": "assistant", "content": null,
            "tool_calls": [
                {"id": "1", "type": "function", "function": {"name": "search", "arguments": "{}"}},
                {"id": "2", "type": "function", "function": {"name": "fetch", "arguments": "{}"}}
            ]
        }));
        assert_eq!(reply_text(&tools_only), "[called search, fetch]");
    }

    #[test]
    fn a_streamed_reply_is_gathered_from_its_chunks() {
        let mut reply = StreamedReply::default();
        reply.push(Some("hel"), None);
        reply.push(Some("lo"), None);
        assert_eq!(reply.text(), "hello");

        let mut tools = StreamedReply::default();
        let call = |name: Option<&str>| ToolCallDelta {
            index: 0,
            function: Some(FunctionCallDelta {
                name: name.map(str::to_string),
                arguments: Some("{".into()),
            }),
            ..Default::default()
        };
        tools.push(None, Some(&[call(Some("search"))]));
        tools.push(None, Some(&[call(None)]));
        assert_eq!(tools.text(), "[called search]");
    }
}
