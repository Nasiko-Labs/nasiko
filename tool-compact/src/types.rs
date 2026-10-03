use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn default_function_kind() -> String {
    "function".to_string()
}

/// Tool definition mirroring the standard function-tool shape.
///
/// Decoupled from `nasiko-llm-router` IR so `tool-compact` remains a pure,
/// standalone library with zero external router dependencies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ToolDef {
    /// Creates a new function tool definition.
    pub fn new(
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

    pub fn name(&self) -> &str {
        &self.function.name
    }

    pub fn description(&self) -> Option<&str> {
        self.function.description.as_deref()
    }

    pub fn parameters(&self) -> Option<&Value> {
        self.function.parameters.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An assistant tool call (standard OpenAI-compatible shape).
///
/// `function.arguments` is stored as a raw JSON string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments serialized as a JSON string.
    pub arguments: String,
}

/// A streamed tool-call delta fragment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

/// The result of tool schema compaction.
///
/// Contains the compact definitions string, calling instructions, and
/// the original schemas to allow schema roundtrip checking (`decode_tools`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Formatted compact schemas for each tool.
    pub definitions: String,
    /// Instructions explaining the call grammar (`<<call ...>>`).
    pub instructions: String,
    /// The source tools, preserved for lossless verification and downstream validation.
    pub tools: Vec<ToolDef>,
}

impl CompactTools {
    /// Returns the complete combined block to inject into the system message.
    pub fn prompt_injection(&self) -> String {
        format!("{}\n\n{}", self.definitions, self.instructions)
    }
}
