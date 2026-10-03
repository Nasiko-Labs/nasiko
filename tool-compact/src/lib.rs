use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum CompactError {
    #[error("unknown_tool")]
    UnknownTool,
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    #[error("parse_error: {0}")]
    ParseError(String),
}

impl CompactError {
    pub fn error_code(&self) -> &'static str {
        match self {
            CompactError::UnknownTool => "unknown_tool",
            CompactError::InvalidArguments(_) => "invalid_arguments",
            CompactError::ParseError(_) => "invalid_arguments",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    #[serde(default = "default_tool_type")]
    pub r#type: String,
    pub function: FunctionDef,
}

fn default_tool_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactTools {
    pub definitions: String,
    pub instructions: String,
    pub combined_prompt: String,
}

/// Compact tool definitions into a dense single-line signature format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut lines = Vec::new();

    for tool in tools {
        let f = &tool.function;
        let mut params_str = Vec::new();
        let mut required_set = HashSet::new();

        if let Some(Value::Object(map)) = &f.parameters {
            let mut req_order = Vec::new();
            if let Some(Value::Array(reqs)) = map.get("required") {
                for r in reqs {
                    if let Some(s) = r.as_str() {
                        required_set.insert(s.to_string());
                        req_order.push(s.to_string());
                    }
                }
            }

            if let Some(Value::Object(props)) = map.get("properties") {
                // Add required properties first in declared required order
                for rk in &req_order {
                    if let Some(spec) = props.get(rk) {
                        let type_str = format_type(spec);
                        params_str.push(format!("{rk}:{type_str}"));
                    }
                }
                // Then add optional properties
                for (name, spec) in props {
                    if !required_set.contains(name) {
                        let type_str = format_type(spec);
                        params_str.push(format!("{name}?:{type_str}"));
                    }
                }
            }
        }

        let desc_part = f
            .description
            .as_deref()
            .map(|d| format!(" - {}", d.trim()))
            .unwrap_or_default();

        lines.push(format!("{}({}){}", f.name, params_str.join(", "), desc_part));
    }

    let definitions = lines.join("\n");
    let instructions = "To call a tool, emit: <<call name {json args}>>".to_string();
    let combined_prompt = if definitions.is_empty() {
        instructions.clone()
    } else {
        format!("{}\n{}", definitions, instructions)
    };

    Ok(CompactTools {
        definitions,
        instructions,
        combined_prompt,
    })
}

fn format_type(spec: &Value) -> String {
    if let Value::Object(m) = spec {
        if let Some(Value::Array(enums)) = m.get("enum") {
            let variants: Vec<String> = enums
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !variants.is_empty() {
                return variants.join("|");
            }
        }

        match m.get("type").and_then(|t| t.as_str()) {
            Some("string") => {
                if let Some(fmt) = m.get("format").and_then(|f| f.as_str()) {
                    if fmt == "date-time" {
                        return "datetime".to_string();
                    }
                    if fmt == "date" {
                        return "date".to_string();
                    }
                }
                "str".to_string()
            }
            Some("integer") => "int".to_string(),
            Some("number") => "float".to_string(),
            Some("boolean") => "bool".to_string(),
            Some("array") => {
                let inner = m.get("items").map(format_type).unwrap_or_else(|| "any".to_string());
                format!("[{inner}]")
            }
            Some("object") => "obj".to_string(),
            _ => "any".to_string(),
        }
    } else {
        "any".to_string()
    }
}

/// Decodes compact tool definitions back into standard ToolDef schemas to verify schema preservation.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();
    for line in compact.definitions.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let open_paren = trimmed
            .find('(')
            .ok_or_else(|| CompactError::ParseError("Missing open paren in definition".into()))?;
        let close_paren = trimmed
            .find(')')
            .ok_or_else(|| CompactError::ParseError("Missing close paren in definition".into()))?;

        let tool_name = trimmed[..open_paren].trim().to_string();
        let params_slice = &trimmed[open_paren + 1..close_paren];
        let after_paren = &trimmed[close_paren + 1..];

        let description = if let Some(dash_idx) = after_paren.find(" - ") {
            let desc = after_paren[dash_idx + 3..].trim();
            if desc.is_empty() {
                None
            } else {
                Some(desc.to_string())
            }
        } else {
            None
        };

        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        if !params_slice.trim().is_empty() {
            for param in params_slice.split(',') {
                let p = param.trim();
                if p.is_empty() {
                    continue;
                }

                let colon_idx = p
                    .find(':')
                    .ok_or_else(|| CompactError::ParseError(format!("Missing colon in parameter '{p}'")))?;
                let name_part = p[..colon_idx].trim();
                let type_part = p[colon_idx + 1..].trim();

                let (name, is_req) = if let Some(stripped) = name_part.strip_suffix('?') {
                    (stripped.to_string(), false)
                } else {
                    (name_part.to_string(), true)
                };

                if is_req {
                    required.push(Value::String(name.clone()));
                }

                let spec = parse_compact_type(type_part);
                properties.insert(name, spec);
            }
        }

        let parameters = serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        });

        tools.push(ToolDef {
            r#type: "function".to_string(),
            function: FunctionDef {
                name: tool_name,
                description,
                parameters: Some(parameters),
            },
        });
    }

    Ok(tools)
}

fn parse_compact_type(type_str: &str) -> Value {
    if type_str.contains('|') {
        let variants: Vec<Value> = type_str
            .split('|')
            .map(|s| Value::String(s.trim().to_string()))
            .collect();
        return serde_json::json!({
            "type": "string",
            "enum": variants
        });
    }

    if let Some(inner) = type_str.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let item_spec = parse_compact_type(inner);
        return serde_json::json!({
            "type": "array",
            "items": item_spec
        });
    }

    match type_str {
        "datetime" => serde_json::json!({
            "type": "string",
            "format": "date-time"
        }),
        "date" => serde_json::json!({
            "type": "string",
            "format": "date"
        }),
        "str" => serde_json::json!({ "type": "string" }),
        "int" => serde_json::json!({ "type": "integer" }),
        "float" => serde_json::json!({ "type": "number" }),
        "bool" => serde_json::json!({ "type": "boolean" }),
        "obj" => serde_json::json!({ "type": "object" }),
        _ => serde_json::json!({ "type": "string" }),
    }
}

/// Decodes LLM output text into verified ToolCalls, validating against original schemas.
/// Uses `serde_json::Deserializer::from_str().into_iter::<Value>()` to accurately calculate
/// the exact byte offset of the arguments JSON object, safely preserving any `>>` inside strings.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut calls = Vec::new();
    let mut cursor = text;

    while let Some(start_idx) = cursor.find("<<call") {
        let after_call = &cursor[start_idx + 6..];
        if !after_call.starts_with(char::is_whitespace) {
            cursor = &cursor[start_idx + 6..];
            continue;
        }

        let trimmed_after = after_call.trim_start();
        if trimmed_after.is_empty() {
            return Err(CompactError::InvalidArguments("Empty tool call after marker".into()));
        }

        // Tool name ends at whitespace, '{', or '>'
        let name_end = trimmed_after
            .find(|c: char| c.is_whitespace() || c == '{' || c == '>')
            .unwrap_or(trimmed_after.len());
        let tool_name = &trimmed_after[..name_end];
        if tool_name.is_empty() {
            return Err(CompactError::InvalidArguments("Missing tool name in call".into()));
        }

        // Fail closed: tool must be in schemas
        let tool_def = tools
            .iter()
            .find(|t| t.function.name == tool_name)
            .ok_or(CompactError::UnknownTool)?;

        let after_name = trimmed_after[name_end..].trim_start();

        let (raw_args, bytes_consumed) = if after_name.starts_with('{') {
            // CRITICAL EDGE CASE: Parse using Deserializer::into_iter to get exact byte offset
            let de = serde_json::Deserializer::from_str(after_name);
            let mut iter = de.into_iter::<Value>();
            let parsed_val = match iter.next() {
                Some(Ok(v)) => v,
                Some(Err(e)) => return Err(CompactError::InvalidArguments(e.to_string())),
                None => return Err(CompactError::InvalidArguments("Empty JSON arguments".into())),
            };
            let offset = iter.byte_offset();
            (parsed_val, offset)
        } else if after_name.starts_with(">>") {
            (Value::Object(serde_json::Map::new()), 0)
        } else {
            return Err(CompactError::InvalidArguments(format!(
                "Invalid arguments syntax for tool '{tool_name}'"
            )));
        };

        let rest_after_json = &after_name[bytes_consumed..];
        let rest_trimmed = rest_after_json.trim_start();
        if !rest_trimmed.starts_with(">>") {
            return Err(CompactError::InvalidArguments(
                "Missing closing >> for tool call".into(),
            ));
        }

        // Validate arguments against original schema
        validate_arguments(&raw_args, tool_def)?;

        calls.push(ToolCall {
            name: tool_name.to_string(),
            arguments: raw_args,
        });

        // Advance cursor past closing ">>"
        let close_pos = rest_after_json.find(">>").unwrap();
        cursor = &rest_after_json[close_pos + 2..];
    }

    Ok(calls)
}

fn validate_arguments(args: &Value, tool: &ToolDef) -> Result<(), CompactError> {
    let obj = args
        .as_object()
        .ok_or_else(|| CompactError::InvalidArguments("Arguments must be a JSON object".into()))?;

    if let Some(Value::Object(param_map)) = &tool.function.parameters {
        if let Some(Value::Array(reqs)) = param_map.get("required") {
            for r in reqs {
                if let Some(key) = r.as_str() {
                    if !obj.contains_key(key) {
                        return Err(CompactError::InvalidArguments(format!(
                            "Missing required field: {key}"
                        )));
                    }
                }
            }
        }

        if let Some(Value::Object(props)) = param_map.get("properties") {
            for (arg_name, arg_val) in obj {
                if let Some(spec) = props.get(arg_name) {
                    validate_type(arg_val, spec)?;
                }
            }
        }
    }

    Ok(())
}

fn validate_type(val: &Value, spec: &Value) -> Result<(), CompactError> {
    if let Value::Object(m) = spec {
        if let Some(Value::Array(enums)) = m.get("enum") {
            if !enums.contains(val) {
                return Err(CompactError::InvalidArguments(format!(
                    "Value {val} not in allowed enum variants"
                )));
            }
        }

        if let Some(expected_type) = m.get("type").and_then(|t| t.as_str()) {
            let ok = match expected_type {
                "string" => val.is_string(),
                "integer" => val.is_i64() || val.is_u64(),
                "number" => val.is_number(),
                "boolean" => val.is_boolean(),
                "array" => {
                    if let Some(arr) = val.as_array() {
                        if let Some(item_spec) = m.get("items") {
                            for item in arr {
                                validate_type(item, item_spec)?;
                            }
                        }
                        true
                    } else {
                        false
                    }
                }
                "object" => {
                    if let Some(nested_obj) = val.as_object() {
                        if let Some(Value::Array(nested_reqs)) = m.get("required") {
                            for req in nested_reqs {
                                if let Some(key) = req.as_str() {
                                    if !nested_obj.contains_key(key) {
                                        return Err(CompactError::InvalidArguments(format!(
                                            "Missing required nested field: {key}"
                                        )));
                                    }
                                }
                            }
                        }
                        if let Some(Value::Object(nested_props)) = m.get("properties") {
                            for (n_key, n_val) in nested_obj {
                                if let Some(n_spec) = nested_props.get(n_key) {
                                    validate_type(n_val, n_spec)?;
                                }
                            }
                        }
                        true
                    } else {
                        false
                    }
                }
                _ => true,
            };

            if !ok {
                return Err(CompactError::InvalidArguments(format!(
                    "Expected {expected_type}, got {val}"
                )));
            }
        }
    }

    Ok(())
}

/// Incremental stream decoder that buffers chunks and handles split markers.
#[derive(Debug, Default, Clone)]
pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn push_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    pub fn finish(self, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
        decode_calls(&self.buffer, tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_calendar_tool() -> ToolDef {
        ToolDef {
            r#type: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Event title" },
                        "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                        "duration_min": { "type": "integer", "description": "Duration in minutes" },
                        "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                        "visibility": { "type": "string", "enum": ["public", "private"] }
                    },
                    "required": ["title", "start"]
                })),
            },
        }
    }

    fn sample_email_tool() -> ToolDef {
        ToolDef {
            r#type: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email to recipients.".into()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        }
    }

    #[test]
    fn test_encode_tools_format() {
        let tools = vec![sample_calendar_tool()];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.definitions.contains("create_calendar_event(title:str, start:datetime, attendees?:[str], duration_min?:int, visibility?:public|private) - Create an event in the user's calendar."));
        assert_eq!(compact.instructions, "To call a tool, emit: <<call name {json args}>>");
        assert!(compact.combined_prompt.contains(&compact.definitions));
        assert!(compact.combined_prompt.contains(&compact.instructions));
    }

    #[test]
    fn test_decode_tools_roundtrip() {
        let tools = vec![sample_calendar_tool()];
        let compact = encode_tools(&tools).unwrap();
        let decoded_tools = decode_tools(&compact).unwrap();
        assert_eq!(decoded_tools.len(), 1);
        let f = &decoded_tools[0].function;
        assert_eq!(f.name, "create_calendar_event");
        assert_eq!(f.description.as_deref(), Some("Create an event in the user's calendar."));

        let params = f.parameters.as_ref().unwrap();
        let reqs = params["required"].as_array().unwrap();
        assert!(reqs.contains(&Value::String("title".into())));
        assert!(reqs.contains(&Value::String("start".into())));
        assert_eq!(params["properties"]["visibility"]["enum"], serde_json::json!(["public", "private"]));
    }

    #[test]
    fn test_decode_calls_single_and_multiple() {
        let tools = vec![sample_calendar_tool(), sample_email_tool()];
        let text = r#"
            I have scheduled the sync and notified Sam.
            <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
            <<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
            All done!
        "#;

        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
        assert_eq!(calls[1].name, "send_email");
        assert_eq!(calls[1].arguments["subject"], "Build status");
    }

    #[test]
    fn test_decode_calls_no_calls() {
        let tools = vec![sample_calendar_tool()];
        let text = "What a pleasant sunny day in Hyderabad!";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_critical_edge_case_arrow_in_string_value() {
        let tools = vec![sample_email_tool()];
        // Note the literal ">>" embedded inside the body string
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"Attention >> Urgent","body":"Please check step 1 >> step 2 >> step 3"}>>"#;

        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[0].arguments["subject"], "Attention >> Urgent");
        assert_eq!(calls[0].arguments["body"], "Please check step 1 >> step 2 >> step 3");
    }

    #[test]
    fn test_stream_decoder_split_chunks() {
        let tools = vec![sample_calendar_tool()];
        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];

        let mut decoder = StreamDecoder::new();
        for chunk in chunks {
            decoder.push_chunk(chunk);
        }

        let calls = decoder.finish(&tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Retro");
        assert_eq!(calls[0].arguments["start"], "2026-10-04T10:00:00+05:30");
    }

    #[test]
    fn test_fail_closed_unknown_tool() {
        let tools = vec![sample_calendar_tool()];
        let text = r#"<<call format_hard_drive {"confirm": true}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err, CompactError::UnknownTool);
        assert_eq!(err.error_code(), "unknown_tool");
    }

    #[test]
    fn test_fail_closed_missing_required_argument() {
        let tools = vec![sample_calendar_tool()];
        // Missing required field 'start'
        let text = r#"<<call create_calendar_event {"title":"Design review"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments(_)));
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_fail_closed_enum_violation() {
        let tools = vec![sample_calendar_tool()];
        // visibility is "secret", but enum is ["public", "private"]
        let text = r#"<<call create_calendar_event {"title":"Confidential","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments(_)));
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_fail_closed_type_mismatch() {
        let tools = vec![sample_calendar_tool()];
        // duration_min should be integer, not string
        let text = r#"<<call create_calendar_event {"title":"Sync","start":"2026-10-05T15:00:00+05:30","duration_min":"thirty"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments(_)));
        assert_eq!(err.error_code(), "invalid_arguments");
    }
}