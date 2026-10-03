//! Seam types for tool definitions and tool calls.

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_function_kind() -> String {
    "function".to_string()
}

/// A tool definition in OpenAI function-calling format.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionDef,
}

/// The function component of a [`ToolDef`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// An assistant tool call emitted by a model or decoder.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    pub function: FunctionCall,
}

/// The function call component of a [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments encoded as a JSON string (OpenAI wire format).
    pub arguments: String,
}

/// Compact representation of tools and instructions to inject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactTools {
    pub definitions: String,
    pub instructions: String,
}

impl CompactTools {
    /// Render the definitions and call instructions into a single prompt block.
    pub fn render(&self) -> String {
        if self.definitions.is_empty() {
            self.instructions.clone()
        } else if self.instructions.is_empty() {
            self.definitions.clone()
        } else {
            format!("{}\n{}", self.definitions, self.instructions)
        }
    }
}
