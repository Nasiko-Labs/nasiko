//! Own `ToolDef` / `ToolCall` types and the [`CompactTools`] encoding result.
//!
//! These mirror the router's `ir::chat` types so the compact crate stays decoupled
//! from `nasiko-llm-router`. The router converts at the seam.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::schema::ToolIR;
use std::collections::BTreeMap;

fn function_kind() -> String {
    "function".to_string()
}

// ── OpenAI-shaped mirrors ──────────────────────────────────────────────────

/// An OpenAI function-tool definition (crate-local mirror).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An assistant tool call (OpenAI shape). `function.arguments` is a JSON **string**.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI's contract).
    pub arguments: String,
}

// ── Compact encoding result ────────────────────────────────────────────────

/// Per-tool compaction status.
#[derive(Debug, Clone)]
pub struct ToolCompactStatus {
    pub name: String,
    pub compacted: bool,
    /// Why the tool was bypassed, if applicable.
    pub reason: Option<String>,
}

/// The result of [`crate::encode_tools`].
///
/// Contains the rendered compact text (for injection as a system message), the
/// per-tool registry (for validation during decoding), and the original tool
/// definitions (for `decode_tools` and bypass pass-through).
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// The rendered compact text to inject into a system message.
    pub text: String,
    /// Whether the overall request was compacted (`false` = all tools bypassed or
    /// compact form was not meaningfully smaller).
    pub compacted: bool,
    /// Per-tool status.
    pub tool_status: Vec<ToolCompactStatus>,
    /// Registry of tool IRs for validation and `decode_tools`.
    pub(crate) registry: BTreeMap<String, ToolIR>,
    /// Original tool definitions (all, for decode_tools and validation).
    pub(crate) original_tools: Vec<ToolDef>,
}
