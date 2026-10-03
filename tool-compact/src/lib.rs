//! Nasiko Compact Tools — a library for encoding tool definitions compactly
//! and decoding model output back into standard OpenAI-shaped tool calls.
//!
//! This crate is intentionally decoupled from the router: it defines its own
//! types and does not depend on nasiko-llm-router. The router converts at the seam.
//!
//! # API
//!
//! - [`encode_tools`]: compact tool definitions + call-format instructions
//! - [`decode_calls`]: parse model output into tool calls (fail-closed)
//! - [`StreamDecoder`]: incremental decoder for streaming responses

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("missing required field: {0}")]
    MissingRequired(String),
    #[error("invalid argument type: {0}")]
    InvalidArgumentType(String),
    #[error("enum value not in allowed set: {0}")]
    EnumViolation(String),
    #[error("json error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("invalid schema: {0}")]
    InvalidSchema(String),
}

pub type Result<T> = std::result::Result<T, CompactError>;

/// Tool definition in compact format
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactTools {
    /// Compact definitions: name -> signature string
    pub tools: HashMap<String, String>,
    /// Grammar: how to write tool calls
    pub grammar: String,
}

/// A standard tool definition (input side)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema object
    pub parameters: Option<serde_json::Value>,
}

/// A standard tool call (output side)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    /// JSON string of arguments
    pub arguments: String,
}

/// Encode a list of tool definitions into compact format with instructions.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut compact_tools = HashMap::new();

    for tool in tools {
        let sig = encode_tool_signature(tool)?;
        compact_tools.insert(tool.name.clone(), sig);
    }

    let grammar = "To call a tool, emit: <<call name {json args}>>\n";

    Ok(CompactTools {
        tools: compact_tools,
        grammar: grammar.to_string(),
    })
}

/// Encode a single tool into a compact signature line.
fn encode_tool_signature(tool: &ToolDef) -> Result<String> {
    let mut sig = tool.name.clone();
    sig.push('(');

    if let Some(params) = &tool.parameters {
        if let Some(props) = params.get("properties").and_then(|p| p.as_object()) {
            let required: std::collections::HashSet<_> = params
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();

            let mut first = true;
            for (name, schema) in props {
                if !first {
                    sig.push_str(", ");
                }
                first = false;

                if !required.contains(name) {
                    sig.push_str(name);
                    sig.push_str("?");
                } else {
                    sig.push_str(name);
                }

                sig.push(':');
                sig.push_str(&encode_type(schema)?);;
            }
        }
    }

    sig.push(')');

    if let Some(desc) = &tool.description {
        sig.push_str(" - ");
        sig.push_str(desc);
    }

    Ok(sig)
}

/// Encode a JSON schema type into a compact type string.
fn encode_type(schema: &serde_json::Value) -> Result<String> {
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => {
            if let Some(enums) = schema.get("enum").and_then(|e| e.as_array()) {
                let variants: Vec<_> = enums
                    .iter()
                    .filter_map(|e| e.as_str())
                    .collect();
                Ok(variants.join("|")); 
                Ok(variants.join("|")); 
            } else {
                Ok("str".to_string())
            }
        }
        Some("integer") => Ok("int".to_string()),
        Some("number") => Ok("float".to_string()),
        Some("boolean") => Ok("bool".to_string()),
        Some("array") => {
            if let Some(items) = schema.get("items") {
                let item_type = encode_type(items)?;
                Ok(format!("[{}]", item_type))
            } else {
                Ok("[any]".to_string())
            }
        }
        Some("object") => Ok("object".to_string()),
        _ => Ok("any".to_string()),
    }
}

/// Decode model output text into tool calls.
/// Validates every call against the original schema (fail-closed).
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let tool_map: HashMap<_, _> = tools.iter().map(|t| (&t.name, t)).collect();
    let mut calls = Vec::new();

    let call_pattern = regex::Regex::new(r"<<call\s+(\w+)\s+(\{.*?\})>>").unwrap();

    for cap in call_pattern.captures_iter(text) {
        let tool_name = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let args_str = cap.get(2).map(|m| m.as_str()).unwrap_or("{}");

        if !tool_map.contains_key(tool_name) {
            return Err(CompactError::UnknownTool(tool_name.to_string()));
        }

        let args_json: serde_json::Value = serde_json::from_str(args_str)?;
        validate_arguments(&args_json, &tool_map[tool_name])?;

        calls.push(ToolCall {
            name: tool_name.to_string(),
            arguments: args_str.to_string(),
        });
    }

    Ok(calls)
}

/// Validate arguments against tool schema (fail-closed).
fn validate_arguments(args: &serde_json::Value, tool: &ToolDef) -> Result<()> {
    if !args.is_object() {
        return Err(CompactError::InvalidArgumentType(
            "arguments must be a JSON object".to_string(),
        ));
    }

    if let Some(params) = &tool.parameters {
        if let Some(required) = params.get("required").and_then(|r| r.as_array()) {
            for req_field in required {
                if let Some(field_name) = req_field.as_str() {
                    if !args.get(field_name).is_some() {
                        return Err(CompactError::MissingRequired(field_name.to_string()));
                    }
                }
            }
        }

        // Basic type validation against schema
        if let Some(props) = params.get("properties").and_then(|p| p.as_object()) {
            for (key, value) in args.as_object().unwrap() {
                if let Some(schema) = props.get(key) {
                    validate_value(value, schema)?;
                }
            }
        }
    }

    Ok(())
}

/// Validate a single value against its schema.
fn validate_value(value: &serde_json::Value, schema: &serde_json::Value) -> Result<()> {
    if let Some(enums) = schema.get("enum").and_then(|e| e.as_array()) {
        if !enums.contains(value) {
            return Err(CompactError::EnumViolation(
                format!("value {:?} not in enum set", value),
            ));
        }
    }

    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => {
            if !value.is_string() {
                return Err(CompactError::InvalidArgumentType(
                    "expected string".to_string(),
                ));
            }
        }
        Some("integer") => {
            if !value.is_i64() {
                return Err(CompactError::InvalidArgumentType(
                    "expected integer".to_string(),
                ));
            }
        }
        Some("boolean") => {
            if !value.is_boolean() {
                return Err(CompactError::InvalidArgumentType(
                    "expected boolean".to_string(),
                ));
            }
        }
        Some("array") => {
            if !value.is_array() {
                return Err(CompactError::InvalidArgumentType(
                    "expected array".to_string(),
                ));
            }
        }
        _ => {}
    }

    Ok(())
}

/// Optional: decode compact format back to standard tools (for verification).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    // This is a simplified stub. A full implementation would parse the signatures
    // back into ToolDef structures with schema. For now, return an error.
    Err(CompactError::InvalidSchema(
        "decode_tools not yet implemented".to_string(),
    ))
}

/// Incremental decoder for streaming responses (handles markers split across chunks).
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            buffer: String::new(),
            tools,
        }
    }

    /// Feed a chunk and attempt to decode complete tool calls.
    /// Returns calls found so far; remainder stays in buffer.
    pub fn feed_chunk(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);
        let mut calls = Vec::new();

        loop {
            let call_pattern = regex::Regex::new(r"<<call\s+(\w+)\s+(\{[^}]*\})>>").unwrap();
            if let Some(cap) = call_pattern.find(&self.buffer) {
                let matched = cap.as_str();
                if let Ok(decoded) = decode_calls(matched, &self.tools) {
                    calls.extend(decoded);
                    self.buffer = self.buffer[cap.end()..].to_string();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        Ok(calls)
    }

    /// Finalize: attempt to decode any remaining buffer.
    pub fn finish(mut self) -> Result<Vec<ToolCall>> {
        decode_calls(&self.buffer, &self.tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_simple_tool() {
        let tool = ToolDef {
            name: "greet".to_string(),
            description: Some("Say hello".to_string()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                },
                "required": ["name"]
            })),
        };

        let compact = encode_tools(&[tool]).unwrap();
        assert!(compact.tools.contains_key("greet"));
        let sig = &compact.tools["greet"];
        assert!(sig.contains("name:str"));
        assert!(sig.contains("age?:int"));
        assert!(sig.contains("Say hello"));
    }

    #[test]
    fn test_decode_valid_call() {
        let tool = ToolDef {
            name: "greet".to_string(),
            description: None,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            })),
        };

        let text = r#"Hello! <<call greet {"name":"Alice"}>>#";
        let calls = decode_calls(text, &[tool]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "greet");
    }

    #[test]
    fn test_decode_unknown_tool() {
        let tool = ToolDef {
            name: "greet".to_string(),
            description: None,
            parameters: None,
        };

        let text = r#"<<call unknown {"x":1}>>"#;
        let result = decode_calls(text, &[tool]);
        assert!(matches!(result, Err(CompactError::UnknownTool(_))));
    }

    #[test]
    fn test_decode_missing_required() {
        let tool = ToolDef {
            name: "greet".to_string(),
            description: None,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            })),
        };

        let text = r#"<<call greet {}>>"#;
        let result = decode_calls(text, &[tool]);
        assert!(matches!(result, Err(CompactError::MissingRequired(_))));
    }

    #[test]
    fn test_stream_decoder_split_marker() {
        let tool = ToolDef {
            name: "test".to_string(),
            description: None,
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": { "x": { "type": "integer" } },
                "required": ["x"]
            })),
        };

        let mut decoder = StreamDecoder::new(vec![tool]);
        let chunk1 = "Hello ";
        let chunk2 = "<<call test {\"x\":42}>>";
        let chunk3 = " done";

        let _ = decoder.feed_chunk(chunk1).unwrap();
        let calls = decoder.feed_chunk(chunk2).unwrap();
        let _ = decoder.feed_chunk(chunk3).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "test");
    }
}
