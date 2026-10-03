use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::encode::HEADER;

/// A function tool, reduced to what compaction needs.
///
/// Deliberately not the router's `ToolDef`: this crate sits below the router, which converts at
/// the seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded call. `arguments` is a JSON **string** (OpenAI's contract) and is exactly what the
/// model wrote — validated, never re-serialized. The caller assigns the call id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// The compact form of a tool list: one definition line per tool, plus how to call them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    /// One line per tool, in the order given.
    pub definitions: String,
    /// The call-format instruction.
    pub instructions: String,
}

impl CompactTools {
    /// The full block to put in front of the model.
    pub fn prompt(&self) -> String {
        format!("{HEADER}\n{}\n{}", self.definitions, self.instructions)
    }
}

/// One piece of decoded model output, in the order it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Ordinary text outside any call. A streaming decoder may split one run of text across
    /// several events.
    Text(String),
    Call(ToolCall),
}

/// Model output separated into its text and its calls.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Decoded {
    /// Everything outside the calls, concatenated.
    pub text: String,
    pub calls: Vec<ToolCall>,
}
