//! Tool-compact's own types — independent of `nasiko-llm-router`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition (mirrors the OpenAI function-tool shape).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call (OpenAI shape: name + JSON-string arguments).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolCall {
    pub name: String,
    /// Arguments as a JSON string (OpenAI contract).
    pub arguments: String,
}

/// The result of encoding tools into compact form.
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// The compact text block describing all tools (one line per tool + call instructions).
    pub text: String,
    /// Number of tools that were compacted.
    pub tool_count: usize,
    /// Tools that were bypassed (unsupported schema features).
    pub bypassed: Vec<String>,
}

/// Schema type extracted from JSON Schema for compact rendering.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SchemaType {
    Str,
    Int,
    Float,
    Bool,
    DateTime,
    Enum(Vec<String>),
    Array(Box<SchemaType>),
    Object(Vec<Param>),
    Any,
}

/// A single parameter extracted from JSON Schema properties.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Param {
    pub name: String,
    pub typ: SchemaType,
    pub required: bool,
    pub description: Option<String>,
}
