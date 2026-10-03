//! Tool definition and call types — deliberately decoupled from `nasiko-llm-router`'s
//! `ir::chat` types so this crate stays a pure library with no router dependency.
//!
//! The router converts at the seam:
//!   - inbound:  `ir::chat::ToolDef`  → `types::ToolDef`   (via `From` impls in the router)
//!   - outbound: `types::ToolCall`     → `ir::chat::ToolCall` (router assigns the call id)

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An OpenAI-shaped function-tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Always `"function"` for now; kept for round-trip correctness.
    #[serde(rename = "type", default = "default_function")]
    pub kind: String,
    pub function: FunctionDef,
}

fn default_function() -> String {
    "function".into()
}

/// The definition of a single callable function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the function's parameters (type = "object").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call produced by [`crate::decode_calls`].
/// `id` is intentionally absent; the router assigns call ids.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    #[serde(rename = "type", default = "default_function")]
    pub kind: String,
    pub function: FunctionCall,
}

/// The function name + arguments pair inside a [`ToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments as a JSON string (OpenAI contract).
    pub arguments: String,
}
