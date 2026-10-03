//! Standalone core data types for compact tool definitions, calls, and streaming events.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Deref;

/// Canonical tool definition with name, optional description, and optional JSON schema parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique name of the tool.
    pub name: String,
    /// Human- or LLM-readable description of what the tool does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's input parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl ToolDefinition {
    /// Creates a new `ToolDefinition`.
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

/// Convenience alias for `ToolDefinition` to match standard ToolDef naming.
pub type ToolDef = ToolDefinition;

/// Encoded compact tools prompt payload and metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactTools {
    /// The formatted compact DSL string ready for system prompt injection.
    pub prompt: String,
    /// Number of tools encoded in this batch.
    pub tools_count: usize,
}

impl CompactTools {
    /// Creates a new `CompactTools` instance.
    pub fn new(prompt: impl Into<String>, tools_count: usize) -> Self {
        Self {
            prompt: prompt.into(),
            tools_count,
        }
    }

    /// Returns the encoded prompt string slice.
    pub fn as_str(&self) -> &str {
        &self.prompt
    }
}

impl Deref for CompactTools {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.prompt
    }
}

impl AsRef<str> for CompactTools {
    fn as_ref(&self) -> &str {
        &self.prompt
    }
}

impl fmt::Display for CompactTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.prompt)
    }
}

impl From<CompactTools> for String {
    fn from(c: CompactTools) -> Self {
        c.prompt
    }
}

/// A decoded tool invocation from model output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Generated unique identifier for this tool call.
    pub id: String,
    /// Name of the invoked tool.
    pub name: String,
    /// Canonical JSON string containing validated arguments.
    pub arguments: String,
}

/// Alias for `ToolCall` to match public naming conventions.
pub type DecodedToolCall = ToolCall;

/// Combined full-text decoding output containing conversational text and extracted tool calls.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DecodeOutput {
    /// The surrounding conversational text (excluding call syntax markers).
    pub text: String,
    /// Successfully parsed and validated tool calls.
    pub tool_calls: Vec<ToolCall>,
}

/// Incremental streaming events emitted by `StreamDecoder`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// Incremental plain text slice.
    TextDelta(String),
    /// A new tool call invocation began.
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Incremental chunk of raw JSON arguments for the active tool call.
    ToolCallArgsDelta { index: usize, delta: String },
    /// A tool call finished and passed fail-closed validation.
    ToolCallComplete { index: usize, call: ToolCall },
}

/// Lossless internal AST representing the supported JSON Schema subset.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeSchema {
    String,
    Integer,
    Number,
    Boolean,
    Null,
    Enum(Vec<String>),
    Array(Box<TypeSchema>),
    Object {
        properties: BTreeMap<String, PropertySchema>,
        required: BTreeSet<String>,
        additional_properties: bool,
    },
}

/// Field/property schema containing its type, documentation, and optional default.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertySchema {
    pub schema: TypeSchema,
    pub description: Option<String>,
    pub default: Option<serde_json::Value>,
}
