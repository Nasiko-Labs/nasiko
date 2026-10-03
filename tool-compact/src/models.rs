use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Provider-independent function definition. Names and descriptions are never rewritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// Validated call; the router assigns the client-facing ID and serializes arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// All information required to reconstruct the definitions is in `definitions`.
/// No hidden copy of the original schemas is retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactTools {
    pub definitions: String,
}

impl CompactTools {
    /// Full system-message content, including the instruction overhead counted by evaluation.
    pub fn prompt(&self) -> String {
        format!("{CALL_INSTRUCTIONS}\n{}", self.definitions)
    }
}

/// Shared by the evaluation and router so the scored prompt is the deployed prompt.
pub const CALL_INSTRUCTIONS: &str = "Tools (? optional): <<call NAME {JSON arguments}>>";

/// Plain response text and validated calls, released together only on successful completion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedOutput {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// Resource bounds; exceeding one returns an error, never a truncated or guessed call.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_output_bytes: usize,
    pub max_call_bytes: usize,
    pub max_calls: usize,
    pub max_json_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_output_bytes: 1_048_576,
            max_call_bytes: 262_144,
            max_calls: 128,
            max_json_depth: 64,
        }
    }
}
