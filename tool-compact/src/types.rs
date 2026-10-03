use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// OpenAI-shaped definition owned by the pure library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    /// V1 accepts only `function`.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// Function metadata and argument schema.
    pub function: FunctionDef,
    /// Unrecognized metadata is retained so preflight can reject it safely.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Function definition; absent/null parameters mean zero arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Exact supplied function name.
    pub name: String,
    /// Model-visible description, preserved verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Original JSON Schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    /// Includes extensions such as `strict`; never silently discarded.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Validated call; integration code assigns provider call IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Exact supplied tool name.
    pub name: String,
    /// JSON object, without coercion or repair.
    pub arguments: Value,
}

/// Exact model-facing representation; no original-schema sidecar.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// Deterministically rendered tool grammar and call syntax.
    pub rendered: String,
}

fn function_kind() -> String {
    "function".into()
}
