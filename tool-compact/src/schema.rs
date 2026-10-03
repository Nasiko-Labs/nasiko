use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool definition used by the compact representation.
///
/// This intentionally does not depend on `nasiko-llm-router`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// Tool call produced by the compact decoder.
///
/// `arguments` is kept as a JSON string to match the OpenAI tool-call
/// contract used by the router.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// Encoded representation of a set of tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactTools {
    pub definitions: String,
}
