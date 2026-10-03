//! Public types of the compact tool-schema codec.
//!
//! The crate owns its own [`ToolDef`] and [`ToolCall`] so it never depends on
//! `nasiko-llm-router`; the router converts at the seam.

use serde_json::Value;

/// A tool definition: name, optional description and a JSON Schema for its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema object describing the arguments.
    pub parameters: Option<Value>,
}

/// A decoded tool call. `arguments` is a JSON string (OpenAI shape); the router assigns ids.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    /// Canonical compact JSON with sorted keys, so output is deterministic.
    pub arguments: String,
}

/// A tool left in its native form because compaction cannot preserve its schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Bypass {
    pub tool: String,
    /// Reason code, for example `unsupported:$ref`.
    pub reason: String,
}

/// Compact rendering of a tool list.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// Call-format instructions followed by one signature line per compacted tool.
    pub text: String,
    /// Tools that were not compacted; empty when every tool was compacted.
    pub bypassed: Vec<Bypass>,
}

/// Why a model output could not be decoded into tool calls. Decoding fails closed:
/// an error is returned instead of a guessed or repaired call.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown_tool: {0}")]
    UnknownTool(String),
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
}

impl DecodeError {
    /// Stable machine-readable code used by the eval output.
    pub fn code(&self) -> &'static str {
        match self {
            DecodeError::UnknownTool(_) => "unknown_tool",
            DecodeError::InvalidArguments(_) => "invalid_arguments",
        }
    }
}

/// Why a tool list could not be encoded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EncodeError {
    #[error("invalid tool: {0}")]
    InvalidTool(String),
}
