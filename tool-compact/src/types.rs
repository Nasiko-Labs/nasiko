//! Core types for the compact tool schema crate.
//!
//! These types are deliberately independent of `nasiko-llm-router`'s IR types.
//! The router converts at the seam using `From` / `Into` implementations.

use serde::{Deserialize, Serialize};

/// A tool definition — mirrors the OpenAI `function` tool shape but owned by this crate.
///
/// The router converts its `ir::chat::ToolDef` → this type before calling `encode_tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool function name (e.g. `"create_calendar_event"`).
    pub name: String,
    /// Human-readable description of what the tool does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's parameters (`{"type": "object", "properties": {...}, ...}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// A decoded tool call — the result of parsing compact model output.
///
/// The router converts this back into its `ir::chat::ToolCall` (assigning `id`, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The tool function name that was called.
    pub name: String,
    /// Parsed arguments as a JSON object.
    pub arguments: serde_json::Value,
}

/// The result of compacting tool definitions.
///
/// Contains the compact text representation, call-format instructions, and the
/// original tool definitions (needed for validation during decoding).
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// Compact text definitions (one line per tool in Python-signature style).
    pub definitions: String,
    /// Instructions telling the LLM how to format tool calls.
    pub call_instructions: String,
    /// The full compact prompt to inject (definitions + instructions).
    pub prompt: String,
    /// Original tool definitions preserved for decode-time validation.
    pub original_tools: Vec<ToolDef>,
}
