use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool definition conforming to native JSON schema function declarations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}

impl ToolDef {
    pub fn new(name: impl Into<String>, description: Option<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
        }
    }
}

/// A parsed and schema-validated tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// Parsed JSON object representing validated arguments.
    pub arguments: Value,
    /// Preserved original JSON substring after validation to avoid unnecessary reserialization.
    pub arguments_json: String,
}

/// Compact representation of tools under CTP/1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    pub protocol_version: String,
    pub text: String,
}

impl CompactTools {
    pub const PROTOCOL_VERSION: &'static str = "ctp/1";

    pub fn new(text: impl Into<String>) -> Self {
        Self {
            protocol_version: Self::PROTOCOL_VERSION.to_string(),
            text: text.into(),
        }
    }
}

/// Decoded response containing assistant text and atomically validated tool calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DecodedResponse {
    /// Non-call text outside <<call ...>> blocks.
    pub text: String,
    /// Atomic validated tool calls in order of appearance.
    pub calls: Vec<ToolCall>,
}
