//! Independent tool types — no dependency on `nasiko-llm-router`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A function-tool definition (OpenAI-shaped, without the `type` wrapper).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call. Arguments are a JSON **object** (not a string).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Structured compact representation of a tool set, plus the prompt text.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// Full prompt block: one signature line per tool, then call instructions.
    pub prompt: String,
    /// Per-tool structured entries (for `decode_tools` / validation).
    pub entries: Vec<CompactToolEntry>,
}

/// One tool's compact signature and reconstructed parameter metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactToolEntry {
    pub name: String,
    pub description: Option<String>,
    /// Compact one-liner: `name(params) - description`
    pub signature: String,
    /// Parameter specs in declaration order (deterministic).
    pub params: Vec<ParamSpec>,
}

/// A single parameter in the compact signature.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub required: bool,
    pub type_expr: TypeExpr,
    pub description: Option<String>,
}

/// Compact type expression (subset of JSON Schema).
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    Str,
    /// `format: date-time` (and similar) — still validated as string.
    Datetime,
    Date,
    Time,
    Uri,
    Int,
    Num,
    Bool,
    Null,
    /// Closed enum of string literals.
    Enum(Vec<String>),
    Array(Box<TypeExpr>),
    /// Nested object: ordered fields.
    Object(Vec<ParamSpec>),
    /// Opaque object with no properties listed (`type: object` only).
    OpaqueObject,
}
