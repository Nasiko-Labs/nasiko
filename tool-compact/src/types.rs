//! Core types for compact tool schemas and calls.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

fn default_function_kind() -> String {
    "function".to_string()
}

/// An OpenAI-compatible function tool definition.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl ToolDef {
    pub fn new(function: FunctionDef) -> Self {
        Self {
            kind: default_function_kind(),
            function,
            extra: Map::new(),
        }
    }
}

/// Function metadata and parameter schema.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

impl FunctionDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters: None,
        }
    }
}

/// A decoded tool call (OpenAI-compatible shape).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

/// Function invocation name and JSON string arguments.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI contract).
    pub arguments: String,
}

/// A streamed fragment of a tool call.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct ToolCallDelta {
    pub index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

/// Streamed function fragment.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct FunctionCallDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// Result of compacting a set of tool definitions.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CompactTools {
    /// Formatted compact definitions in TOON notation.
    pub definitions: String,
    /// Concise system instructions on how the LLM should invoke tools.
    pub instructions: String,
}

impl CompactTools {
    /// Combines instructions and tool definitions into a single prompt block.
    pub fn prompt_block(&self) -> String {
        format!("{}\n\nTools:\n{}", self.instructions, self.definitions)
    }
}

/// Encoding error.
#[derive(Debug, Error, PartialEq)]
pub enum EncodeError {
    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Decoding and validation error.
#[derive(Debug, Error, PartialEq, Clone)]
pub enum DecodeError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("malformed call syntax: {0}")]
    MalformedSyntax(String),
    #[error("unsupported syntax: {0}")]
    Unsupported(String),
}

impl DecodeError {
    /// Returns the standard error string expected by evaluation harnesses ("unknown_tool" | "invalid_arguments").
    pub fn eval_error_kind(&self) -> &'static str {
        match self {
            DecodeError::UnknownTool(_) => "unknown_tool",
            DecodeError::InvalidArguments(_)
            | DecodeError::MalformedSyntax(_)
            | DecodeError::Unsupported(_) => "invalid_arguments",
        }
    }
}
