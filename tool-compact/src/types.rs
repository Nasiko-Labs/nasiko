use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn default_function_kind() -> String {
    "function".to_string()
}

/// Function definition containing name, description, and JSON Schema parameters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// Function tool definition matching the standard OpenAI/router schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl ToolDef {
    pub fn new_function(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: FunctionDef {
                name: name.into(),
                description,
                parameters,
            },
            extra: Map::new(),
        }
    }
}

/// A parsed function call.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments formatted as a serialized JSON string.
    pub arguments: String,
}

/// Standard tool call output matching OpenAI format.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten, default)]
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
            kind: "function".to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
            extra: Map::new(),
        }
    }
}

/// Incremental delta for streaming responses.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolCallDelta {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

/// Compact representation of tools with calling instructions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CompactTools {
    /// Concise compact tool signatures (e.g., `func(a:str, b?:int)`).
    pub signatures: String,
    /// Detailed prompt instructions injected into the system prompt.
    pub instructions: String,
    /// System prompt section combining signatures and instructions.
    pub prompt_section: String,
    /// Original tools kept for roundtripping and validation.
    pub original_tools: Vec<ToolDef>,
}
