//! Public types for the compact tool encoding.

use serde::{Deserialize, Serialize};

/// A tool definition, independent of any router IR.
///
/// `parameters` is a JSON Schema (object schema) for the tool's arguments, in the
/// same shape as OpenAI's `function.parameters`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<serde_json::Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
        }
    }
}

/// One decoded tool call: the tool name plus its arguments as a JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// Always a JSON object.
    pub arguments: serde_json::Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

/// The result of [`crate::encode_tools`].
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// One signature line per tool, in input order, joined by `\n`.
    /// Bypassed tools (unsupported schema) render as `name(?) - description`.
    pub rendered: String,
    /// The full instruction block to inject into the prompt: call-format rules plus
    /// the signature lines.
    pub instructions: String,
    /// Per-tool, in input order: `true` when the tool was compacted, `false` when it
    /// was bypassed because its schema is outside the supported subset.
    pub compacted: Vec<bool>,
}

/// Fail-closed decode/encode errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The call names a tool that is not in the tool list.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// The marker parsed but the arguments do not satisfy the tool's schema
    /// (missing required field, wrong type, bad enum value, non-object args…).
    #[error("invalid arguments for tool {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },
    /// A `<<call` marker was opened but never completed, or its shape is broken.
    #[error("malformed tool call marker: {0}")]
    Malformed(String),
    /// A tool's JSON Schema is outside the supported subset (refs, combinators,
    /// non-string enums, over-deep nesting…). The tool must be bypassed, not guessed.
    #[error("unsupported schema for tool {tool}: {reason}")]
    UnsupportedSchema { tool: String, reason: String },
}

impl Error {
    /// The snake_case code used in eval outputs.
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnknownTool(_) => "unknown_tool",
            Error::InvalidArguments { .. } => "invalid_arguments",
            Error::Malformed(_) => "malformed",
            Error::UnsupportedSchema { .. } => "unsupported_schema",
        }
    }
}
