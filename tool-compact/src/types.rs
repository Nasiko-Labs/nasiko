//! Core types for nasiko-tool-compact.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Definition of a tool, matching standard JSON Schema function signatures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

impl ToolDef {
    /// Create a new tool definition.
    pub fn new(
        name: impl Into<String>,
        description: Option<String>,
        parameters: Option<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description,
            parameters,
        }
    }

    /// Parse a tool definition from a JSON Value, accepting either OpenAI `{ "type": "function", "function": {...} }`
    /// or flat `{ "name": ..., "description": ..., "parameters": ... }`.
    pub fn from_value(val: &Value) -> Result<Self, CompactError> {
        if let Some(func) = val.get("function") {
            let name = func
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| CompactError::Malformed("Tool function missing 'name'".to_string()))?
                .to_string();
            let description = func
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let parameters = func.get("parameters").cloned();
            Ok(Self {
                name,
                description,
                parameters,
            })
        } else if let Some(name) = val.get("name").and_then(Value::as_str) {
            let description = val
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let parameters = val.get("parameters").cloned();
            Ok(Self {
                name: name.to_string(),
                description,
                parameters,
            })
        } else {
            Err(CompactError::Malformed(
                "Invalid tool definition value".to_string(),
            ))
        }
    }
}

/// A parsed tool call representing a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

/// Compact representation of tools to inject into system prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// The formatted compact tool signatures.
    pub text: String,
    /// List of tool names included.
    pub tool_names: Vec<String>,
    /// Instructions for the model to use the grammar.
    pub instructions: String,
    /// Complete combined prompt (instructions + signatures).
    pub full_prompt: String,
}

/// Errors produced during tool compaction, schema decoding, or call decoding.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CompactError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("Unsupported schema feature for tool '{tool}': {feature}")]
    Unsupported { tool: String, feature: String },

    #[error("Malformed compact syntax: {0}")]
    Malformed(String),
}

/// Call-format instruction variants for prompt ablation and tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstructionVariant {
    #[default]
    Concise,
    Detailed,
    Minimal,
}

impl InstructionVariant {
    pub fn text(&self) -> &'static str {
        match self {
            Self::Concise => {
                "To invoke a tool, output exactly <<call tool_name {\"arg\": \"val\"}>>. \
Arguments must strictly be valid JSON matching the tool schema. \
Multiple calls are permitted sequentially. \
If no tool applies, output your plain answer without any <<call ...>> syntax."
            }
            Self::Detailed => {
                "## Tool Calling Instructions\n\
You have access to tools defined above. To call a tool, use the exact syntax:\n\
<<call tool_name {\"param\": value}>>\n\
Rules:\n\
1. Arguments must strictly adhere to valid JSON conforming to the parameter types.\n\
2. You may invoke multiple tools consecutively.\n\
3. Any text outside <<call ...>> blocks is presented directly to the user.\n\
4. If no tool is needed to answer the request, do not use the <<call ...>> syntax; respond directly."
            }
            Self::Minimal => {
                "Call tools via <<call tool_name {\"arg\": val}>>. Strict JSON. Multiple calls allowed."
            }
        }
    }
}
