use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn default_function_kind() -> String {
    "function".to_string()
}

/// An OpenAI-compatible tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ToolDef {
    pub fn new_function(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self {
            kind: default_function_kind(),
            function: FunctionDef {
                name: name.into(),
                description,
                parameters,
            },
            extra: Map::new(),
        }
    }
}

/// Details of a function within a tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An OpenAI-compatible assistant tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: default_function_kind(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
            extra: Map::new(),
        }
    }
}

/// A function call within a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments formatted as a JSON string (OpenAI contract).
    pub arguments: String,
}

/// Output of compact tool encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Full instructions block ready for prompt/system injection.
    pub prompt: String,
    /// Individual compact signatures per tool.
    pub signatures: Vec<String>,
    /// Calling format instructions.
    pub instructions: String,
    /// List of tool names that were compacted.
    pub tool_names: Vec<String>,
}
