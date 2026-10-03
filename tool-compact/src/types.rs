//! The crate's own tool types, mirroring the OpenAI Chat Completions shape.
//!
//! Deliberately not the router's IR types: the router depends on this crate, so it converts
//! at the seam. Field names and serde shapes match `llm-router/src/ir/chat.rs`, which makes
//! that conversion a field-by-field copy.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}

/// An OpenAI function-tool definition.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    /// Unknown top-level fields. Anything here cannot be represented compactly, so a
    /// non-empty map makes [`crate::encode_tools`] report the tool as unsupported.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

impl ToolDef {
    /// Convenience constructor for a `function` tool.
    pub fn function(name: &str, description: Option<&str>, parameters: Option<Value>) -> Self {
        Self {
            kind: function_kind(),
            function: FunctionDef {
                name: name.to_string(),
                description: description.map(str::to_string),
                parameters,
            },
            extra: Map::new(),
        }
    }
}

/// A decoded tool call. `arguments` is a JSON **string** (OpenAI's contract), byte-identical
/// to the object the model wrote — the decoder validates it but never rewrites it.
///
/// There is no `id`: the router assigns ids when it converts to its own `ToolCall`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    /// The arguments parsed as JSON. Always succeeds for a call produced by the decoder.
    pub fn arguments_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}
