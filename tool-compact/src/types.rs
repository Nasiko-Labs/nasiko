use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn function_kind() -> String {
    "function".to_string()
}

/// OpenAI-shaped function tool definition.
///
/// Field names match the router IR so a tool list can cross the crate boundary
/// without a second JSON dialect. `extra` keeps unknown sibling fields so a
/// round trip does not strip vendor extensions the caller still needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool arguments. Absent means the tool takes no arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// OpenAI-shaped tool call. `arguments` is a JSON object serialized as a string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Result of [`crate::encode_tools`].
///
/// `compacted` is false when any tool uses a schema this crate cannot represent
/// without dropping information. In that case `prompt` is empty and the caller
/// must send the original tool list unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub prompt: String,
    pub compacted: bool,
    /// Set when `compacted` is false. Stable for a given schema.
    pub bypass_reason: Option<String>,
}

impl ToolCall {
    pub(crate) fn new(index: usize, name: String, arguments: String) -> Self {
        Self {
            id: format!("call_{index}"),
            kind: function_kind(),
            function: FunctionCall { name, arguments },
            extra: Map::new(),
        }
    }
}
