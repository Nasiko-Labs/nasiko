//! Shared types for the compact-tools library.
//!
//! These are deliberately independent of `nasiko-llm-router` IR. The router converts
//! at the seam (`ToolDef` / `ToolCall` ↔ IR equivalents).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition (name + optional description + JSON Schema parameters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for arguments (`type: "object"`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call in OpenAI shape: `arguments` is a JSON **string**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI's contract).
    pub arguments: String,
}

/// Result of [`crate::encode_tools`]: prompt text plus structured schemas for round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Text injected into the model prompt (definitions + call-format instructions).
    pub prompt: String,
    /// Structured schemas preserved for [`crate::decode_tools`] / validation.
    pub(crate) tools: Vec<ToolDef>,
}

impl CompactTools {
    /// The prompt fragment to inject (definitions + call-format instructions).
    pub fn as_str(&self) -> &str {
        &self.prompt
    }

    /// Tools that were successfully compacted.
    pub fn tools(&self) -> &[ToolDef] {
        &self.tools
    }
}

/// Failures from encode / decode / stream. Never invent a call on error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactError {
    /// Schema uses a feature this encoder does not support — caller should bypass.
    #[error("unsupported schema feature: {0}")]
    UnsupportedSchema(String),
    /// Tool name is empty or not a compact identifier.
    #[error("invalid tool name: {0}")]
    InvalidToolName(String),
    /// Model named a tool that is not in the provided set.
    #[error("unknown_tool")]
    UnknownTool,
    /// Arguments failed JSON parse or schema validation.
    #[error("invalid_arguments")]
    InvalidArguments,
    /// Compact call marker / grammar was malformed.
    #[error("malformed_call")]
    MalformedCall,
    /// A previous stream error left the decoder closed.
    #[error("decoder_closed")]
    DecoderClosed,
}

impl CompactError {
    /// Stable wire label for eval / scorer (`unknown_tool`, `invalid_arguments`, …).
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::UnknownTool => "unknown_tool",
            Self::InvalidArguments => "invalid_arguments",
            Self::MalformedCall => "malformed_call",
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::InvalidToolName(_) => "invalid_tool_name",
            Self::DecoderClosed => "decoder_closed",
        }
    }
}
