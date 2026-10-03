//! Mirror types for `ToolDef`, `FunctionDef`, `ToolCall`, `FunctionCall`
//! (mirrors `llm-router/src/ir/chat.rs` without depending on that crate).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}

/// An OpenAI function-tool definition.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDef {
    /// Always `"function"` in the OpenAI wire protocol.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// The function specification.
    pub function: FunctionDef,
    /// Pass-through for unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The function part of a [`ToolDef`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FunctionDef {
    /// Unique name used in calls.
    pub name: String,
    /// Human-readable description shown to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An assistant tool call in the OpenAI wire protocol.
///
/// `function.arguments` is a JSON **string**, not a parsed object.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolCall {
    /// Call identifier (the router assigns a real UUID; the library leaves a placeholder).
    pub id: String,
    /// Always `"function"`.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// The function invocation.
    pub function: FunctionCall,
    /// Pass-through for unknown fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The function invocation inside a [`ToolCall`].
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct FunctionCall {
    /// Name of the function called.
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI's wire contract).
    pub arguments: String,
}
