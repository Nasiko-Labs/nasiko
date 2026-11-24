//! Rebuilds a session from the coding-agent telemetry the platform stores per turn
//! (`coding_agent_telemetry_events.payload`, a `CodingAgentEventV1`), for harvesting on the
//! server. Only the fields used here are read, and every one is optional, so the crate does
//! not depend on the platform's event types.
//!
//! Typed messages come from each turn's `prompt`, edits from Edit / Write / MultiEdit tool
//! calls' `arguments` (the coding agent's raw tool input), and usage from each model call,
//! keyed by the provider's message id so a reply split over several calls counts once.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::{FileEdit, ModelUsage, SessionError, SessionRecord, UserMessage};
use crate::cost::TokenUsage;
use crate::instance::SessionSource;

/// Where the session ran, as the platform recorded it with the session.
#[derive(Debug, Clone, Default)]
pub struct SessionPlace {
    /// The coding agent's session id (not the platform's scoped id).
    pub source_session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

/// Builds the session from its turns' payloads, in any order.
pub fn from_payloads(
    place: &SessionPlace,
    payloads: &[Value],
) -> Result<SessionRecord, SessionError> {
    if place.source_session_id.is_empty() {
        return Err(SessionError::NoSessionId);
    }
    let mut turns: Vec<Turn> = payloads
        .iter()
        .filter_map(|p| serde_json::from_value::<Event>(p.clone()).ok())
        .map(|e| e.turn)
        .collect();
    turns.sort_by_key(|t| t.started_at);
    let (Some(first), Some(last)) = (turns.first(), turns.iter().map(|t| t.ended_at).max()) else {
        return Err(SessionError::NoTimestamps);
    };
    let started_at = first.started_at;
    let mut user_messages = Vec::new();
    let mut edits = Vec::new();
    let mut usage = Vec::new();
    let mut seen = BTreeSet::new();
    for turn in &turns {
        if let Some(text) = turn
            .prompt
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            user_messages.push(UserMessage {
                at: turn.started_at,
                text: text.to_string(),
            });
        }
        for tool in &turn.tool_calls {
            edits.extend(file_edits(tool, turn.started_at));
        }
        for call in &turn.llm_calls {
            let id = call
                .accounting
                .as_ref()
                .and_then(|a| a.message_id.clone())
                .unwrap_or_else(|| call.id.clone());
            if !seen.insert(id.clone()) {
                continue;
            }
            usage.push(ModelUsage {
                at: call.started_at.unwrap_or(turn.started_at),
                message_id: id,
                model: call.model.clone(),
                usage: TokenUsage {
                    input: call.input_tokens,
                    output: call.output_tokens,
                    cache_write: call.cache_creation_tokens,
                    cache_read: call.cache_read_tokens,
                    reasoning: 0,
                },
            });
        }
    }
    edits.sort_by_key(|e: &FileEdit| e.at);
    Ok(SessionRecord {
        session_id: place.source_session_id.clone(),
        source: SessionSource::ClaudeCode,
        cwd: place.cwd.clone().unwrap_or_default(),
        git_branch: place.git_branch.clone().filter(|b| !b.is_empty()),
        started_at,
        ended_at: last,
        user_messages,
        edits,
        usage,
    })
}

/// Whether a turn payload carries content (prompts and tool arguments), i.e. it was synced
/// with chat history on.
pub fn has_content(payload: &Value) -> bool {
    payload.get("capture_policy").and_then(Value::as_str) == Some("content")
}

fn file_edits(tool: &ToolCall, turn_start: DateTime<Utc>) -> Vec<FileEdit> {
    let Some(args) = tool.arguments.as_ref() else {
        return Vec::new();
    };
    let Some(path) = args.get("file_path").and_then(Value::as_str) else {
        return Vec::new();
    };
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let pairs: Vec<(String, String)> = match tool.name.as_str() {
        "Edit" => vec![(text(args, "old_string"), text(args, "new_string"))],
        "Write" => vec![(String::new(), text(args, "content"))],
        "MultiEdit" => args
            .get("edits")
            .and_then(Value::as_array)
            .map(|edits| {
                edits
                    .iter()
                    .map(|e| (text(e, "old_string"), text(e, "new_string")))
                    .collect()
            })
            .unwrap_or_default(),
        _ => return Vec::new(),
    };
    let at = tool.started_at.unwrap_or(turn_start);
    pairs
        .into_iter()
        .map(|(old, new)| {
            let before: BTreeSet<&str> = old.lines().collect();
            FileEdit {
                at,
                path: path.to_string(),
                added_lines: new
                    .lines()
                    .filter(|l| !before.contains(l))
                    .map(str::to_string)
                    .collect(),
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct Event {
    turn: Turn,
}

#[derive(Debug, Deserialize)]
struct Turn {
    prompt: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    #[serde(default)]
    llm_calls: Vec<LlmCall>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Deserialize)]
struct LlmCall {
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    accounting: Option<Accounting>,
    started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct Accounting {
    message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolCall {
    name: String,
    arguments: Option<Value>,
    started_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn turn(id: &str, at: &str, prompt: Option<&str>, tools: Value, calls: Value) -> Value {
        json!({
            "version": 1, "event_id": id, "captured_at": at, "capture_policy": "content",
            "source": {"agent_id": "a", "agent_name": "claude"},
            "session": {"id": "s", "source_id": "sess-1"},
            "turn": {"id": id, "prompt": prompt, "started_at": at, "ended_at": at,
                     "llm_calls": calls, "tool_calls": tools}
        })
    }

    fn call(id: &str, message: &str, at: &str, out: u64) -> Value {
        json!({"id": id, "provider": "anthropic", "model": "claude-opus", "input_tokens": 5,
               "output_tokens": out, "cache_read_tokens": 1, "cache_creation_tokens": 2,
               "accounting": {"version": 1, "message_id": message},
               "started_at": at, "ended_at": at})
    }

    #[test]
    fn rebuilds_messages_edits_and_usage() {
        let payloads = vec![
            turn(
                "t2",
                "2026-11-01T10:10:00Z",
                Some("now the test"),
                json!([{"id": "x", "name": "Write", "kind": "tool", "status": "succeeded",
                        "association": "exact", "timestamp_quality": "exact",
                        "started_at": "2026-11-01T10:11:00Z",
                        "arguments": {"file_path": "/r/tests/t.rs", "content": "fn t() {}\n"}}]),
                json!([call("c3", "msg-2", "2026-11-01T10:10:30Z", 7)]),
            ),
            turn(
                "t1",
                "2026-11-01T10:00:00Z",
                Some("  make b return 2 "),
                json!([
                    {"id": "e", "name": "Edit", "kind": "tool", "status": "succeeded",
                     "association": "exact", "timestamp_quality": "exact",
                     "arguments": {"file_path": "/r/src/lib.rs",
                                   "old_string": "fn a() {}\nfn b() -> u8 { 1 }",
                                   "new_string": "fn a() {}\nfn b() -> u8 { 2 }"}},
                    {"id": "m", "name": "MultiEdit", "kind": "tool", "status": "succeeded",
                     "association": "exact", "timestamp_quality": "exact",
                     "arguments": {"file_path": "/r/src/m.rs",
                                   "edits": [{"old_string": "x", "new_string": "y"}]}},
                    {"id": "b", "name": "Bash", "kind": "tool", "status": "succeeded",
                     "association": "exact", "timestamp_quality": "exact",
                     "arguments": {"command": "git commit"}}
                ]),
                // Two calls for one reply (same message id) count once.
                json!([
                    call("c1", "msg-1", "2026-11-01T10:00:10Z", 3),
                    call("c2", "msg-1", "2026-11-01T10:00:20Z", 3)
                ]),
            ),
        ];
        let place = SessionPlace {
            source_session_id: "sess-1".into(),
            cwd: Some("/r".into()),
            git_branch: Some("main".into()),
        };
        let s = from_payloads(&place, &payloads).expect("rebuilds");
        assert_eq!(s.session_id, "sess-1");
        assert_eq!(s.cwd, "/r");
        assert_eq!(
            s.started_at,
            "2026-11-01T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        let texts: Vec<_> = s.user_messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["make b return 2", "now the test"]);
        let paths: Vec<_> = s.edits.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["/r/src/lib.rs", "/r/src/m.rs", "/r/tests/t.rs"]);
        assert_eq!(s.edits[0].added_lines, vec!["fn b() -> u8 { 2 }"]);
        assert_eq!(s.usage.len(), 2);
        assert_eq!(s.total_usage().output, 10);
        assert_eq!(
            s.edited_paths_in(&["/r".into()]),
            vec!["src/lib.rs", "src/m.rs", "tests/t.rs"]
        );
        assert!(has_content(&payloads[0]));
    }

    #[test]
    fn no_turns_is_an_error() {
        let place = SessionPlace {
            source_session_id: "s".into(),
            ..SessionPlace::default()
        };
        assert_eq!(from_payloads(&place, &[]), Err(SessionError::NoTimestamps));
        assert_eq!(
            from_payloads(&SessionPlace::default(), &[]),
            Err(SessionError::NoSessionId)
        );
    }
}
