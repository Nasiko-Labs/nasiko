use serde::{Deserialize, Serialize};

fn default_tool_kind() -> String {
    "function".to_string()
}

/// Canonical tool definition matching OpenAI function calling contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,
    pub function: FunctionDef,
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<serde_json::Value>,
    ) -> Self {
        Self {
            kind: default_tool_kind(),
            function: FunctionDef {
                name: name.into(),
                description,
                parameters,
            },
        }
    }
}

/// Function definition containing schema details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// Emitted tool call matching standard OpenAI tool call structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: default_tool_kind(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// Function call with serialized JSON arguments string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Result of compacting a set of tool definitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Combined compact prompt ready for injection (schema DSL + call instruction).
    pub prompt: String,
    /// Pure compact schema DSL definitions (one line per tool).
    pub schema_dsl: String,
    /// Call instruction emitted to the model.
    pub instruction: String,
    /// List of tool names that were compacted.
    pub tool_names: Vec<String>,
}
