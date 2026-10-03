//! Public types for the nasiko-tool-compact crate.
//!
//! These are deliberately independent of `nasiko-llm-router` types;
//! conversion happens at the integration seam.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition (OpenAI-compatible shape, self-contained).
///
/// Mirrors `nasiko_llm_router::ir::chat::ToolDef` but belongs to this crate
/// so the library has zero dependency on the router.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    /// Always `"function"`.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// The function specification.
    pub function: FunctionDef,
}

/// Function-level definition inside a [`ToolDef`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    /// The function name, used as the call identifier in compact syntax.
    pub name: String,
    /// Optional human-readable description. Preserved in compact form when
    /// it disambiguates tools with similar parameter sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema object describing the function's parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An opaque handle to the compacted representation of a set of tool definitions.
///
/// Created by [`encode_tools`][crate::encode_tools] and consumed by
/// [`decode_calls`][crate::decode_calls] and [`decode_tools`][crate::decode_tools].
#[derive(Debug, Clone)]
pub struct CompactTools {
    /// Human-readable compact schema text to inject into the system prompt.
    pub schema_text: String,
    /// Grammar reference to include in the system prompt.
    pub grammar_hint: String,
    /// Original definitions, retained for call validation.
    pub(crate) defs: Vec<ToolDef>,
}

impl CompactTools {
    /// Returns the full system-prompt block: schema + grammar hint.
    pub fn system_prompt_block(&self) -> String {
        format!("{}\n\n{}", self.schema_text, self.grammar_hint)
    }
}

/// A successfully decoded and validated tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// The function name.
    pub name: String,
    /// Validated and typed arguments as a JSON object.
    pub arguments: Value,
}

fn function_kind() -> String {
    "function".to_string()
}
