use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum CompactError {
    #[error("Unknown tool: {0}")]
    UnknownTool(String),
    #[error("Missing required argument: {0}")]
    MissingRequired(String),
    #[error("Invalid argument for '{key}': expected {expected}, got {got:?}")]
    InvalidArgument {
        key: String,
        expected: String,
        got: Value,
    },
    #[error("JSON decode error: {0}")]
    JsonError(String),
    #[error("Syntax error: {0}")]
    SyntaxError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    #[serde(default = "default_tool_type")]
    pub kind: String,
    pub function: FunctionDef,
}

fn default_tool_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default = "default_tool_type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactTools {
    pub instructions: String,
    pub compact_definitions: String,
}

fn format_schema_type(prop: &Value) -> String {
    if let Some(enum_vals) = prop.get("enum").and_then(|v| v.as_array()) {
        let items: Vec<String> = enum_vals
            .iter()
            .filter_map(|e| e.as_str().map(|s| s.to_string()))
            .collect();
        if !items.is_empty() {
            return items.join("|");
        }
    }

    let ty = prop.get("type").and_then(|v| v.as_str()).unwrap_or("any");
    let format = prop.get("format").and_then(|v| v.as_str());

    match ty {
        "string" if format == Some("date-time") => "datetime".to_string(),
        "string" => "str".to_string(),
        "integer" => "int".to_string(),
        "number" => "float".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            let item_type = prop
                .get("items")
                .map(format_schema_type)
                .unwrap_or_else(|| "any".to_string());
            format!("[{}]", item_type)
        }
        "object" => "object".to_string(),
        _ => "any".to_string(),
    }
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut defs = Vec::new();

    for tool in tools {
        let func = &tool.function;
        let mut params_str = Vec::new();

        if let Some(params) = &func.parameters {
            let empty_set = HashSet::new();
            let required: HashSet<&str> = params
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or(empty_set);

            if let Some(props) = params.get("properties").and_then(|v| v.as_object()) {
                for (name, prop) in props {
                    let is_req = required.contains(name.as_str());
                    let type_repr = format_schema_type(prop);
                    if is_req {
                        params_str.push(format!("{}:{}", name, type_repr));
                    } else {
                        params_str.push(format!("{}?:{}", name, type_repr));
                    }
                }
            }
        }

        let desc = func
            .description
            .as_ref()
            .map(|d| format!(" - {}", d.trim()))
            .unwrap_or_default();

        defs.push(format!("{}({}){}", func.name, params_str.join(", "), desc));
    }

    Ok(CompactTools {
        instructions: "To call a tool, emit: <<call name {json args}>>".to_string(),
        compact_definitions: defs.join("\n"),
    })
}

fn validate_arguments(
    args: &Map<String, Value>,
    schema: Option<&Value>,
) -> Result<(), CompactError> {
    let Some(schema) = schema else {
        return Ok(());
    };

    if let Some(req_arr) = schema.get("required").and_then(|v| v.as_array()) {
        for req in req_arr {
            if let Some(req_field) = req.as_str() {
                if !args.contains_key(req_field) {
                    return Err(CompactError::MissingRequired(req_field.to_string()));
                }
            }
        }
    }

    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (key, val) in args {
            if let Some(prop_schema) = props.get(key) {
                if let Some(enum_vals) = prop_schema.get("enum").and_then(|e| e.as_array()) {
                    if !enum_vals.contains(val) {
                        return Err(CompactError::InvalidArgument {
                            key: key.clone(),
                            expected: format!("one of {:?}", enum_vals),
                            got: val.clone(),
                        });
                    }
                }

                if let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str()) {
                    let valid = match expected_type {
                        "string" => val.is_string(),
                        "integer" => val.is_i64() || val.is_u64(),
                        "number" => val.is_number(),
                        "boolean" => val.is_boolean(),
                        "array" => val.is_array(),
                        "object" => val.is_object(),
                        _ => true,
                    };

                    if !valid {
                        return Err(CompactError::InvalidArgument {
                            key: key.clone(),
                            expected: expected_type.to_string(),
                            got: val.clone(),
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

fn extract_json_body(input: &str) -> Option<(&str, usize)> {
    let mut in_string = false;
    let mut escape = false;
    let mut brace_depth = 0;
    let mut start_idx = None;

    for (i, c) in input.char_indices() {
        if start_idx.is_none() {
            if c == '{' {
                start_idx = Some(i);
                brace_depth = 1;
            }
            continue;
        }

        if escape {
            escape = false;
            continue;
        }

        if c == '\\' {
            escape = true;
            continue;
        }

        if c == '"' {
            in_string = !in_string;
            continue;
        }

        if !in_string {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth -= 1;
                if brace_depth == 0 {
                    let start = start_idx.unwrap();
                    let end = i + c.len_utf8();
                    let remainder = &input[end..];
                    if let Some(marker_offset) = remainder.find(">>") {
                        return Some((&input[start..end], end + marker_offset + 2));
                    }
                }
            }
        }
    }
    None
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    let mut call_counter = 1;

    while let Some(start_tag) = text[cursor..].find("<<call") {
        let abs_start = cursor + start_tag + 6;
        let content_after = text[abs_start..].trim_start();

        let Some(name_end) = content_after.find(|c: char| c.is_whitespace() || c == '{') else {
            break;
        };

        let tool_name = &content_after[..name_end];
        let matched_tool = tools
            .iter()
            .find(|t| t.function.name == tool_name)
            .ok_or_else(|| CompactError::UnknownTool(tool_name.to_string()))?;

        let body_slice = &content_after[name_end..];
        let (json_str, consumed_len) = extract_json_body(body_slice)
            .ok_or_else(|| CompactError::SyntaxError("Unclosed tool call block".to_string()))?;

        let val: Value = serde_json::from_str(json_str)
            .map_err(|e| CompactError::JsonError(e.to_string()))?;

        let obj = val.as_object().ok_or_else(|| {
            CompactError::InvalidArgument {
                key: "root".to_string(),
                expected: "object".to_string(),
                got: val.clone(),
            }
        })?;

        validate_arguments(obj, matched_tool.function.parameters.as_ref())?;

        calls.push(ToolCall {
            id: format!("call_{}", call_counter),
            kind: "function".to_string(),
            function: FunctionCall {
                name: tool_name.to_string(),
                arguments: json_str.to_string(),
            },
        });

        call_counter += 1;
        cursor = abs_start + (body_slice.as_ptr() as usize - content_after.as_ptr() as usize) + consumed_len;
    }

    Ok(calls)
}

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

    pub fn push_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    pub fn finish(self) -> Result<Vec<ToolCall>, CompactError> {
        decode_calls(&self.buffer, &self.tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: "create_calendar_event".to_string(),
                    description: Some("Create an event".to_string()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "title": {"type": "string"},
                            "start": {"type": "string", "format": "date-time"},
                            "visibility": {"type": "string", "enum": ["public", "private"]}
                        },
                        "required": ["title", "start"]
                    })),
                },
            },
        ]
    }

    #[test]
    fn test_streaming_split_marker_roundtrip() {
        let tools = test_tools();
        let mut decoder = StreamDecoder::new(tools);
        decoder.push_chunk("<<ca");
        decoder.push_chunk("ll create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>");
        decoder.push_chunk(">");

        let calls = decoder.finish().expect("Should decode successfully");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
    }

    #[test]
    fn test_escaping_brackets_in_string() {
        let tools = test_tools();
        let text = r#"<<call create_calendar_event {"title":"Design >> review","start":"2026-10-04T10:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools).expect("Escaped >> inside string must parse");
        assert_eq!(calls.len(), 1);
        assert!(calls[0].function.arguments.contains("Design >> review"));
    }

    #[test]
    fn test_fail_closed_on_enum_violation() {
        let tools = test_tools();
        let text = r#"<<call create_calendar_event {"title":"Test","start":"2026-10-04T10:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools);
        assert!(matches!(err, Err(CompactError::InvalidArgument { .. })));
    }
}
