//! Standalone type definitions for tool schemas and calls.
//!
//! Structurally identical to `nasiko-llm-router::ir::chat`'s `ToolDef`/`ToolCall`
//! but owned by this crate so the public API has no dependency on the router.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}

// ─── Wire types (serde-compatible with ir::chat) ───────────────────────────

/// An OpenAI function-tool definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The function block inside a [`ToolDef`].
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

/// The function block inside a [`ToolCall`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments serialized as a JSON string (OpenAI's contract).
    pub arguments: String,
}

// ─── Internal representations ──────────────────────────────────────────────

/// The result of encoding tools into compact format.
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// The compact textual representation (system-message injectable).
    pub text: String,
    /// Schema metadata retained for decode-time validation.
    pub schemas: Vec<ToolSchema>,
    /// Optional content-addressable ID for caching across turns.
    pub compact_id: Option<String>,
}

/// Parsed schema metadata for one tool, used during decoding to validate calls.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub required_fields: Vec<String>,
    pub optional_fields: Vec<String>,
    pub field_types: HashMap<String, FieldType>,
    pub enum_values: HashMap<String, Vec<String>>,
    /// Original JSON Schema parameters, kept for round-trip [`crate::decode_tools`].
    pub original_parameters: Option<Value>,
    pub description: Option<String>,
    /// `true` when the schema was too complex for compaction (e.g. `oneOf`).
    pub bypassed: bool,
}

/// Simplified type tag derived from a JSON Schema property.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    String,
    Number,
    Integer,
    Boolean,
    Array(Box<FieldType>),
    Object,
    Enum(Vec<String>),
    /// Schema too complex to represent compactly — the tool was bypassed.
    Unknown,
}

impl std::fmt::Display for FieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldType::String => write!(f, "str"),
            FieldType::Number => write!(f, "num"),
            FieldType::Integer => write!(f, "int"),
            FieldType::Boolean => write!(f, "bool"),
            FieldType::Array(inner) => write!(f, "[{inner}]"),
            FieldType::Object => write!(f, "{{...}}"),
            FieldType::Enum(values) => write!(f, "{}", values.join("|")),
            FieldType::Unknown => write!(f, "any"),
        }
    }
}
