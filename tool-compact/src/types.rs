//! Core data models for tool definitions, compact representations, and tool calls.
//!
//! These types are self-contained and deliberately independent of `nasiko-llm-router`.
//! They serialize to and from standard OpenAI-compatible JSON specifications.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn default_function_kind() -> String {
    "function".to_string()
}

/// A tool definition holding a function specification and extra fields.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolDef {
    /// Tool kind, defaults to `"function"`.
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    /// Function metadata and JSON Schema parameters.
    pub function: FunctionDef,
    /// Unknown or extra provider-specific properties preserved verbatim.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl ToolDef {
    /// Create a new `ToolDef` with standard `"function"` kind.
    pub fn new(function: FunctionDef) -> Self {
        Self {
            kind: default_function_kind(),
            function,
            extra: Map::new(),
        }
    }
}

/// Metadata and parameter schema for a callable tool function.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionDef {
    /// The unique name of the function/tool.
    pub name: String,
    /// Optional human-readable description of what the tool does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's input arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded tool call (OpenAI format).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolCall {
    /// Unique identifier for the call (e.g. `"call_1"`).
    pub id: String,
    /// Tool kind, defaults to `"function"`.
    #[serde(rename = "type", default = "default_function_kind")]
    pub kind: String,
    /// The called function name and serialized JSON arguments string.
    pub function: FunctionCall,
    /// Extra metadata preserved verbatim.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

impl ToolCall {
    /// Create a new function tool call with a generated id.
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: impl Into<String>) -> Self {
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

/// The invocation payload for a tool call.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionCall {
    /// Name of the invoked tool.
    pub name: String,
    /// Arguments formatted as a serialized JSON string, matching OpenAI's contract.
    pub arguments: String,
}

/// The result of compacting a set of tool definitions.
///
/// Contains the compact instructions and prompt signatures injected into the LLM context,
/// along with the original tool schemas used later for decoding validation.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CompactTools {
    /// Injected system/user prompt instructions defining the tools and calling grammar.
    pub prompt_text: String,
    /// Preserved tool definitions used by the decoder to validate incoming arguments.
    pub tools: Vec<ToolDef>,
    /// Estimated token count or byte size of the compact representation.
    #[serde(default)]
    pub compact_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_def_serde() {
        let json_data = json!({
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get current weather",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": { "type": "string" }
                    },
                    "required": ["location"]
                }
            }
        });

        let tool_def: ToolDef = serde_json::from_value(json_data.clone()).unwrap();
        assert_eq!(tool_def.kind, "function");
        assert_eq!(tool_def.function.name, "get_weather");
        assert_eq!(
            tool_def.function.description.as_deref(),
            Some("Get current weather")
        );

        let roundtrip = serde_json::to_value(&tool_def).unwrap();
        assert_eq!(roundtrip["function"]["name"], "get_weather");
        assert_eq!(roundtrip["type"], "function");
    }

    #[test]
    fn test_tool_call_serde() {
        let call = ToolCall::new("call_42", "send_email", r#"{"to":"test@example.com"}"#);
        assert_eq!(call.id, "call_42");
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.name, "send_email");
        assert_eq!(call.function.arguments, r#"{"to":"test@example.com"}"#);

        let val = serde_json::to_value(&call).unwrap();
        assert_eq!(val["id"], "call_42");
        assert_eq!(val["type"], "function");
        assert_eq!(val["function"]["name"], "send_email");
        assert_eq!(
            val["function"]["arguments"],
            r#"{"to":"test@example.com"}"#
        );
    }

    #[test]
    fn test_compact_tools_struct() {
        let tool = ToolDef::new(FunctionDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        });

        let compact = CompactTools {
            prompt_text: "ping() - ping server".into(),
            tools: vec![tool.clone()],
            compact_bytes: 20,
        };

        assert_eq!(compact.tools.len(), 1);
        assert_eq!(compact.tools[0].function.name, "ping");
        assert_eq!(compact.compact_bytes, 20);
    }
}
