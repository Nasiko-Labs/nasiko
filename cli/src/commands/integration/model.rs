//! Agent-independent representation of one completed coding-agent session.

use chrono::{DateTime, Utc};
use nasiko_types::coding_agent::{
    CODING_AGENT_ID_MAX_BYTES, CODING_AGENT_NAME_MAX_BYTES, CodingAgentSessionParent,
};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct LlmCall {
    pub uuid: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub accounting: Option<nasiko_types::CodingAgentCallAccounting>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub uuid: String,
    pub prompt: String,
    pub response: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub calls: Vec<LlmCall>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub model_call_id: Option<String>,
    pub status: CodingAgentToolCallStatus,
    pub arguments: Option<Value>,
    pub output: Option<Value>,
    pub raw: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub association: CodingAgentToolAssociation,
    pub timestamp_quality: CodingAgentTimestampQuality,
}

impl Turn {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[derive(Debug)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub title: Option<String>,
    pub turns: Vec<Turn>,
    /// The session that spawned this one, when it is a subagent's. Its turns are its own: they are never
    /// added into the parent's.
    pub parent: Option<CodingAgentSessionParent>,
}

/// The link from subagent session `child_id` to the session `parent_id` that spawned it, kept to what
/// the event contract accepts so a link can never cost a turn: no link to an empty or over-long parent
/// id or to the session itself, and a spawning tool call or a subagent kind the contract would refuse
/// is left out (a kind too long is cut to fit).
pub fn subagent_parent(
    child_id: &str,
    parent_id: &str,
    tool_call_id: Option<&str>,
    agent_type: Option<&str>,
) -> Option<CodingAgentSessionParent> {
    let source_id = parent_id.trim();
    if source_id.is_empty() || source_id.len() > CODING_AGENT_ID_MAX_BYTES || source_id == child_id
    {
        return None;
    }
    Some(CodingAgentSessionParent {
        source_id: source_id.to_string(),
        tool_call_id: tool_call_id
            .filter(|id| !id.trim().is_empty() && id.len() <= CODING_AGENT_ID_MAX_BYTES)
            .map(str::to_string),
        agent_type: agent_type
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .map(|kind| truncated(kind, CODING_AGENT_NAME_MAX_BYTES)),
    })
}

fn truncated(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subagent_link_keeps_only_what_the_contract_accepts() {
        let link =
            subagent_parent("child", " parent ", Some("toolu_1"), Some(" Explore ")).unwrap();
        assert_eq!(link.source_id, "parent");
        assert_eq!(link.tool_call_id.as_deref(), Some("toolu_1"));
        assert_eq!(link.agent_type.as_deref(), Some("Explore"));

        assert!(subagent_parent("same", "same", None, None).is_none());
        assert!(subagent_parent("child", "  ", None, None).is_none());
        assert!(
            subagent_parent(
                "child",
                &"p".repeat(CODING_AGENT_ID_MAX_BYTES + 1),
                None,
                None
            )
            .is_none()
        );

        let long_call = "c".repeat(CODING_AGENT_ID_MAX_BYTES + 1);
        let long_kind = "é".repeat(CODING_AGENT_NAME_MAX_BYTES);
        let link = subagent_parent("child", "parent", Some(&long_call), Some(&long_kind)).unwrap();
        assert_eq!(link.tool_call_id, None);
        let kind = link.agent_type.unwrap();
        assert!(kind.len() <= CODING_AGENT_NAME_MAX_BYTES && kind.starts_with('é'));
        assert_eq!(
            subagent_parent("child", "parent", Some(" "), Some(""))
                .unwrap()
                .tool_call_id,
            None
        );
    }
}
