use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

/// Error types for tool compaction, streaming decoding, and fail-closed validation.
#[derive(Debug, Error, PartialEq, Clone, Serialize, Deserialize)]
pub enum ToolCompactError {
    #[error("unknown tool: '{0}'. Call was rejected (fail-closed policy).")]
    UnknownTool(String),

    #[error("invalid arguments for tool '{0}': {1}")]
    InvalidArguments(String, String),

    #[error("malformed compact syntax: {0}")]
    MalformedSyntax(String),

    #[error("schema decoding error: {0}")]
    SchemaError(String),

    #[error("json parsing error: {0}")]
    JsonError(String),
}

pub type Result<T> = std::result::Result<T, ToolCompactError>;

fn default_tool_kind() -> String {
    "function".to_string()
}

/// Canonical tool definition matching Nasiko's IR (\`llm-router/src/ir/chat.rs\`).
/// Permissive: keeps an \`extra\` map so unknown OpenAI fields pass through untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Function definition with name, description, and JSON Schema parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// Decoded standard tool call (OpenAI shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Extracted function call with name and JSON string arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments formatted as a standard JSON string.
    pub arguments: String,
}

/// Result of encoding tools into compact format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Concise tool signatures in TypeScript-style micro-syntax.
    pub compact_definitions: String,
    /// Explicit system instructions on how the LLM must emit tool calls.
    pub call_instructions: String,
    /// Number of tools encoded.
    pub tool_count: usize,
}
