use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::OnceLock;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum CompactError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("schema error: {0}")]
    SchemaError(String),
    #[error("parse error: {0}")]
    ParseError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactTools {
    pub prompt_injection: String,
}

static CALL_REGEX: OnceLock<Regex> = OnceLock::new();

fn get_call_regex() -> &'static Regex {
    CALL_REGEX.get_or_init(|| {
        Regex::new(r"<<call\s+([a-zA-Z0-9_\-]+)\s+(\{.*?\})>>").expect("invalid regex")
    })
}

/// Formats native JSON schema types into a concise signature
pub fn format_type(schema: &Value) -> String {
    if let Some(enum_vals) = schema.get("enum").and_then(|v| v.as_array()) {
        let items: Vec<String> = enum_vals
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !items.is_empty() {
            return items.join("|");
        }
    }
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => {
            if schema.get("format").and_then(|f| f.as_str()) == Some("date-time") {
                "datetime".into()
            } else {
                "str".into()
            }
        }
        Some("integer") => "int".into(),
        Some("number") => "float".into(),
        Some("boolean") => "bool".into(),
        Some("array") => {
            let item_t = schema
                .get("items")
                .map(format_type)
                .unwrap_or_else(|| "any".into());
            format!("[{}]", item_t)
        }
        Some("object") => "obj".into(),
        _ => "any".into(),
    }
}

/// Encodes JSON schema tools into compact signature lines
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut lines = Vec::new();
    for tool in tools {
        let mut param_sigs = Vec::new();
        let required_fields: Vec<String> = tool
            .parameters
            .as_ref()
            .and_then(|p| p.get("required"))
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if let Some(props) = tool
            .parameters
            .as_ref()
            .and_then(|p| p.get("properties"))
            .and_then(|p| p.as_object())
        {
            for (name, schema) in props {
                let is_req = required_fields.contains(name);
                let ty = format_type(schema);
                if is_req {
                    param_sigs.push(format!("{}:{}", name, ty));
                } else {
                    param_sigs.push(format!("{}?:{}", name, ty));
                }
            }
        }

        let desc = tool
            .description
            .as_deref()
            .map(|d| format!(" - {}", d.trim()))
            .unwrap_or_default();

        lines.push(format!("{}({}){}", tool.name, param_sigs.join(", "), desc));
    }

    lines.push("\nTo call a tool, emit: <<call name {json args}>>".to_string());
    Ok(CompactTools {
        prompt_injection: lines.join("\n"),
    })
}

/// Fail-closed argument and schema validator[cite: 4]
pub fn validate_call(call: &ToolCall, tools: &[ToolDef]) -> Result<(), CompactError> {
    let tool = tools
        .iter()
        .find(|t| t.name == call.name)
        .ok_or_else(|| CompactError::UnknownTool(call.name.clone()))?;

    let args = call
        .arguments
        .as_object()
        .ok_or_else(|| CompactError::InvalidArguments("args not an object".into()))?;

    if let Some(params) = &tool.parameters {
        if let Some(req) = params.get("required").and_then(|r| r.as_array()) {
            for r in req {
                if let Some(k) = r.as_str() {
                    if !args.contains_key(k) {
                        return Err(CompactError::InvalidArguments(format!(
                            "missing required field: {}",
                            k
                        )));
                    }
                }
            }
        }

        if let Some(props) = params.get("properties").and_then(|p| p.as_object()) {
            for (key, val) in args {
                if let Some(prop_schema) = props.get(key) {
                    if let Some(enums) = prop_schema.get("enum").and_then(|e| e.as_array()) {
                        if !enums.contains(val) {
                            return Err(CompactError::InvalidArguments(format!(
                                "invalid enum value for field {}",
                                key
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Decodes non-streaming compact tool calls[cite: 4]
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let re = get_call_regex();
    let mut calls = Vec::new();

    for caps in re.captures_iter(text) {
        let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
        let raw_args = caps.get(2).map_or("", |m| m.as_str());

        let args: Value = serde_json::from_str(raw_args)
            .map_err(|e| CompactError::InvalidArguments(e.to_string()))?;

        let call = ToolCall {
            name,
            arguments: args,
        };
        validate_call(&call, tools)?;
        calls.push(call);
    }
    Ok(calls)
}

/// Incremental streaming decoder capable of parsing markers split across chunks[cite: 4]
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    buffer: String,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef]) -> Self {
        Self {
            tools,
            buffer: String::new(),
        }
    }

    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buffer.push_str(chunk);
        let mut calls = Vec::new();

        while let Some(start) = self.buffer.find("<<call") {
            if let Some(end_rel) = self.buffer[start..].find(">>") {
                let end = start + end_rel + 2;
                let candidate = self.buffer[start..end].to_string();
                let decoded = decode_calls(&candidate, self.tools)?;
                calls.extend(decoded);
                self.buffer.drain(..end);
            } else {
                break;
            }
        }
        Ok(calls)
    }

    pub fn finish(mut self) -> Result<Vec<ToolCall>, CompactError> {
        let calls = decode_calls(&self.buffer, self.tools)?;
        Ok(calls)
    }
}