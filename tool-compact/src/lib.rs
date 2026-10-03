//! `nasiko-tool-compact` — Compact tool schemas and streaming decoder.
//!
//! Provides a token-efficient representation for function tools and a resilient,
//! fail-closed streaming decoder for model responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Public error type adhering to the hackathon screening and evaluation contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unknown_tool")]
    UnknownTool(String),

    #[error("invalid_arguments")]
    InvalidArguments(String),

    #[error("malformed_call")]
    MalformedCall(String),

    #[error("schema_error")]
    SchemaError(String),
}

impl CompactError {
    /// Return the standardized error code used by the evaluation harness.
    pub fn error_code(&self) -> &'static str {
        match self {
            CompactError::UnknownTool(_) => "unknown_tool",
            CompactError::InvalidArguments(_) => "invalid_arguments",
            CompactError::MalformedCall(_) => "malformed_call",
            CompactError::SchemaError(_) => "invalid_schema",
        }
    }
}

/// Standalone tool definition (decoupled from the router IR).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// Decoded tool call representation matching OpenAI-compatible tool calling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Compact representation of tool definitions along with injection text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    /// Formatted tool signatures (one per line).
    pub tools_text: String,
    /// Call-format instruction for models.
    pub call_instructions: String,
    /// Complete text to inject into the system prompt.
    pub prompt_injection: String,
    /// Parsed schema metadata for roundtrip verification.
    pub tools: Vec<ToolDef>,
}

/// Encode full JSON-schema tools into a compact signature format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut sig_lines = Vec::new();

    for tool in tools {
        let sig = encode_single_tool(tool)?;
        sig_lines.push(sig);
    }

    let tools_text = sig_lines.join("\n");
    let call_instructions = "To call a tool, emit: <<call name {\"param\": value}>>".to_string();
    let prompt_injection = format!(
        "Available tools:\n{}\n\n{}",
        tools_text, call_instructions
    );

    Ok(CompactTools {
        tools_text,
        call_instructions,
        prompt_injection,
        tools: tools.to_vec(),
    })
}

fn encode_single_tool(tool: &ToolDef) -> Result<String, CompactError> {
    let mut params_str = String::new();

    if let Some(Value::Object(param_obj)) = &tool.parameters {
        let empty_vec = Vec::new();
        let required_list: Vec<&str> = param_obj
            .get("required")
            .and_then(Value::as_array)
            .unwrap_or(&empty_vec)
            .iter()
            .filter_map(Value::as_str)
            .collect();

        let required_set: HashSet<&str> = required_list.iter().copied().collect();

        if let Some(Value::Object(props)) = param_obj.get("properties") {
            let mut parts = Vec::new();

            // 1. Required fields first in order
            for req_name in &required_list {
                if let Some(prop_val) = props.get(*req_name) {
                    let type_str = format_type(prop_val);
                    parts.push(format!("{}:{}", req_name, type_str));
                }
            }

            // 2. Optional fields next
            for (prop_name, prop_val) in props {
                if !required_set.contains(prop_name.as_str()) {
                    let type_str = format_type(prop_val);
                    parts.push(format!("{}?:{}", prop_name, type_str));
                }
            }

            params_str = parts.join(", ");
        }
    }

    let desc = match &tool.description {
        Some(d) if !d.trim().is_empty() => format!(" - {}", d.trim()),
        _ => String::new(),
    };

    Ok(format!("{}({}){}", tool.name, params_str, desc))
}

fn format_type(val: &Value) -> String {
    if let Value::Object(map) = val {
        if let Some(Value::Array(enums)) = map.get("enum") {
            let enum_vals: Vec<&str> = enums.iter().filter_map(Value::as_str).collect();
            if !enum_vals.is_empty() {
                return enum_vals.join("|");
            }
        }

        if let Some(format) = map.get("format").and_then(Value::as_str) {
            if format == "date-time" {
                return "datetime".to_string();
            }
        }

        if let Some(type_val) = map.get("type").and_then(Value::as_str) {
            match type_val {
                "string" => return "str".to_string(),
                "integer" => return "int".to_string(),
                "number" => return "num".to_string(),
                "boolean" => return "bool".to_string(),
                "array" => {
                    if let Some(items) = map.get("items") {
                        return format!("[{}]", format_type(items));
                    }
                    return "[any]".to_string();
                }
                "object" => return "object".to_string(),
                _ => return type_val.to_string(),
            }
        }
    }
    "any".to_string()
}

/// Decode the compact tool representation back into schema definitions for validation.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    Ok(compact.tools.clone())
}

/// Parse and validate tool calls from raw model text.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new();
    decoder.feed(text);
    decoder.finish(tools)
}

/// State machine streaming decoder that incrementally processes model output chunks.
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

    /// Feed a streaming chunk into the decoder.
    pub fn feed(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// Complete decoding and validate all extracted calls against provided tools.
    pub fn finish(&self, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
        let tools_map: HashMap<&str, &ToolDef> = tools.iter().map(|t| (t.name.as_str(), t)).collect();
        let raw_calls = extract_calls(&self.buffer)?;
        let mut validated_calls = Vec::new();
        let mut first_error = None;

        for (name, args_val) in raw_calls {
            let result = (|| {
                let tool_def = tools_map
                    .get(name.as_str())
                    .ok_or_else(|| CompactError::UnknownTool(name.clone()))?;

                validate_arguments(tool_def, &args_val)?;
                Ok(ToolCall {
                    name,
                    arguments: args_val,
                })
            })();

            match result {
                Ok(call) => validated_calls.push(call),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(validated_calls)
    }
}

/// Robust JSON-aware parser for `<<call tool_name {...}>>`.
/// Handles `>>` inside JSON string literals and nested brackets.
fn extract_calls(text: &str) -> Result<Vec<(String, Value)>, CompactError> {
    let mut results = Vec::new();
    let mut first_error = None;
    let marker = "<<call";
    let mut cursor = 0;
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    while cursor < len {
        // Search for "<<call"
        let window: String = chars[cursor..std::cmp::min(cursor + marker.len(), len)]
            .iter()
            .collect();
        if window != marker {
            cursor += 1;
            continue;
        }

        let call_start = cursor;

        // Check delimiter after "<<call"
        cursor += marker.len();
        if cursor >= len || !chars[cursor].is_whitespace() {
            continue;
        }

        // Skip whitespace before tool name
        while cursor < len && chars[cursor].is_whitespace() {
            cursor += 1;
        }

        // Extract tool name
        let name_start = cursor;
        while cursor < len && !chars[cursor].is_whitespace() && chars[cursor] != '{' {
            cursor += 1;
        }
        let tool_name: String = chars[name_start..cursor].iter().collect();
        if tool_name.is_empty() {
            first_error.get_or_insert(CompactError::MalformedCall(
                "Missing tool name in call".to_string(),
            ));
            cursor = next_marker(&chars, call_start + marker.len(), marker);
            continue;
        }

        // Skip whitespace to JSON start '{'
        while cursor < len && chars[cursor].is_whitespace() {
            cursor += 1;
        }

        if cursor >= len || chars[cursor] != '{' {
            first_error.get_or_insert(CompactError::MalformedCall(
                "Expected '{' for tool arguments".to_string(),
            ));
            cursor = next_marker(&chars, call_start + marker.len(), marker);
            continue;
        }

        // Parse balanced JSON object
        let json_start = cursor;
        let mut brace_depth = 0;
        let mut in_string = false;
        let mut is_escaped = false;
        let mut json_end = None;

        while cursor < len {
            let ch = chars[cursor];

            if in_string {
                if is_escaped {
                    is_escaped = false;
                } else if ch == '\\' {
                    is_escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
            } else {
                match ch {
                    '"' => in_string = true,
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            json_end = Some(cursor + 1);
                            cursor += 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            cursor += 1;
        }

        let json_end_pos = match json_end {
            Some(pos) => pos,
            None => {
                first_error.get_or_insert(CompactError::MalformedCall(
                    "Unclosed JSON in tool call".to_string(),
                ));
                cursor = next_marker(&chars, call_start + marker.len(), marker);
                continue;
            }
        };

        // Skip whitespace after JSON
        while cursor < len && chars[cursor].is_whitespace() {
            cursor += 1;
        }

        // Expect closing ">>"
        if cursor + 1 < len && chars[cursor] == '>' && chars[cursor + 1] == '>' {
            cursor += 2;
        } else {
            first_error.get_or_insert(CompactError::MalformedCall(
                "Missing closing '>>'".to_string(),
            ));
            cursor = next_marker(&chars, call_start + marker.len(), marker);
            continue;
        }

        let json_str: String = chars[json_start..json_end_pos].iter().collect();
        let args_value: Value = match serde_json::from_str(&json_str) {
            Ok(value) => value,
            Err(error) => {
                first_error.get_or_insert(CompactError::InvalidArguments(format!(
                    "JSON parse error: {}",
                    error
                )));
                continue;
            }
        };

        results.push((tool_name, args_value));
    }

    if let Some(error) = first_error {
        return Err(error);
    }

    Ok(results)
}

fn next_marker(chars: &[char], start: usize, marker: &str) -> usize {
    chars[start..]
        .windows(marker.chars().count())
        .position(|window| window.iter().copied().eq(marker.chars()))
        .map(|offset| start + offset)
        .unwrap_or(chars.len())
}

/// Fail-closed argument validation against the JSON Schema parameters.
fn validate_arguments(tool: &ToolDef, args: &Value) -> Result<(), CompactError> {
    let args_obj = match args {
        Value::Object(map) => map,
        _ => {
            return Err(CompactError::InvalidArguments(
                "Tool arguments must be a JSON object".to_string(),
            ));
        }
    };

    if let Some(Value::Object(param_schema)) = &tool.parameters {
        // Validate required fields
        if let Some(Value::Array(req_arr)) = param_schema.get("required") {
            for req_val in req_arr {
                if let Some(req_field) = req_val.as_str() {
                    if !args_obj.contains_key(req_field) {
                        return Err(CompactError::InvalidArguments(format!(
                            "Missing required argument: {}",
                            req_field
                        )));
                    }
                }
            }
        }

        // Validate properties: enums, types
        if let Some(Value::Object(props_schema)) = param_schema.get("properties") {
            for (arg_key, arg_val) in args_obj {
                if let Some(Value::Object(field_schema)) = props_schema.get(arg_key) {
                    // Check enum constraint
                    if let Some(Value::Array(allowed_enums)) = field_schema.get("enum") {
                        let matches_enum = allowed_enums.iter().any(|e| e == arg_val);
                        if !matches_enum {
                            return Err(CompactError::InvalidArguments(format!(
                                "Value for field '{}' does not match allowed enum values",
                                arg_key
                            )));
                        }
                    }

                    // Check type constraint
                    if let Some(expected_type) = field_schema.get("type").and_then(Value::as_str) {
                        match expected_type {
                            "string" => {
                                if !arg_val.is_string() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected string",
                                        arg_key
                                    )));
                                }
                            }
                            "integer" => {
                                if !arg_val.is_i64() && !arg_val.is_u64() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected integer",
                                        arg_key
                                    )));
                                }
                            }
                            "number" => {
                                if !arg_val.is_number() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected number",
                                        arg_key
                                    )));
                                }
                            }
                            "boolean" => {
                                if !arg_val.is_boolean() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected boolean",
                                        arg_key
                                    )));
                                }
                            }
                            "array" => {
                                if !arg_val.is_array() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected array",
                                        arg_key
                                    )));
                                }
                            }
                            "object" => {
                                if !arg_val.is_object() {
                                    return Err(CompactError::InvalidArguments(format!(
                                        "Field '{}' expected object",
                                        arg_key
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create an event in the user's calendar.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer"},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "send_email".to_string(),
                description: Some("Send an email from the user's account.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        ]
    }

    #[test]
    fn test_encode_tools() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.tools_text.contains("create_calendar_event("));
        assert!(compact.tools_text.contains("title:str, start:datetime"));
        assert!(compact.tools_text.contains("visibility?:public|private"));
    }

    #[test]
    fn test_decode_valid_single_call() {
        let tools = sample_tools();
        let text = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
    }

    #[test]
    fn test_marker_split_across_stream_chunks() {
        let tools = sample_tools();
        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ];
        let mut decoder = StreamDecoder::new();
        for c in chunks {
            decoder.feed(c);
        }
        let calls = decoder.finish(&tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Retro");
    }

    #[test]
    fn test_marker_inside_string_literal() {
        let tools = sample_tools();
        let text = "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[0].arguments["subject"], "a >> b");
    }

    #[test]
    fn test_unknown_tool() {
        let tools = sample_tools();
        let text = "<<call delete_everything {}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "unknown_tool");
    }

    #[test]
    fn test_missing_required_and_invalid_enum() {
        let tools = sample_tools();
        let text = "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_no_calls() {
        let tools = sample_tools();
        let text = "What is the weather today in Bengaluru?";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_single_byte_streaming_chunks() {
        let tools = sample_tools();
        let full_text = "<<call create_calendar_event {\"title\":\"Char-by-char split\",\"start\":\"2026-10-04T12:00:00+05:30\"}>>";
        let mut decoder = StreamDecoder::new();
        // Feed 1 character at a time
        for ch in full_text.chars() {
            let mut s = String::new();
            s.push(ch);
            decoder.feed(&s);
        }
        let calls = decoder.finish(&tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Char-by-char split");
    }

    #[test]
    fn test_multiple_calls_with_interleaved_text() {
        let tools = sample_tools();
        let text = "I am scheduling your events now:\n\
                    <<call create_calendar_event {\"title\":\"Sync 1\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>\n\
                    And sending the notification email:\n\
                    <<call send_email {\"to\":[\"team@example.com\"],\"subject\":\"Invite\",\"body\":\"Join us\"}>>\n\
                    All done!";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[1].name, "send_email");
    }

    #[test]
    fn test_escaped_quotes_and_brackets_inside_strings() {
        let tools = sample_tools();
        let text = r#"<<call send_email {"to":["alice@example.com"],"subject":"Quote: \"Hello, world!\" >> nested {braces}","body":"Done."}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(
            calls[0].arguments["subject"],
            r#"Quote: "Hello, world!" >> nested {braces}"#
        );
    }

    #[test]
    fn test_unicode_and_emojis() {
        let tools = sample_tools();
        let text = "<<call create_calendar_event {\"title\":\"🚀 Hackathon Demo — नमस्ते\",\"start\":\"2026-10-05T18:00:00+05:30\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], "🚀 Hackathon Demo — नमस्ते");
    }

    #[test]
    fn test_schema_preservation_roundtrip() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).unwrap();
        let decoded = decode_tools(&compact).unwrap();
        assert_eq!(decoded.len(), tools.len());
        assert_eq!(decoded[0].name, tools[0].name);
        assert_eq!(decoded[1].name, tools[1].name);
    }

    #[test]
    fn test_empty_parameters() {
        let tools = vec![ToolDef {
            name: "ping".to_string(),
            description: Some("Check availability".to_string()),
            parameters: None,
        }];
        let compact = encode_tools(&tools).unwrap();
        assert_eq!(compact.tools_text, "ping() - Check availability");
        assert_eq!(decode_calls("<<call ping {}>>", &tools).unwrap().len(), 1);
    }

    #[test]
    fn test_tool_with_no_description() {
        let tools = vec![ToolDef {
            name: "ping".to_string(),
            description: None,
            parameters: None,
        }];
        assert_eq!(encode_tools(&tools).unwrap().tools_text, "ping()");
    }

    #[test]
    fn test_nested_object_parameter() {
        let tools = vec![ToolDef {
            name: "configure".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "settings": {
                        "type": "object",
                        "properties": {"enabled": {"type": "boolean"}}
                    }
                },
                "required": ["settings"]
            })),
        }];
        assert_eq!(encode_tools(&tools).unwrap().tools_text, "configure(settings:object)");
        let calls = decode_calls("<<call configure {\"settings\":{\"enabled\":true}}>>", &tools).unwrap();
        assert_eq!(calls[0].arguments["settings"]["enabled"], true);
    }

    #[test]
    fn test_multiple_calls_same_tool() {
        let tools = vec![ToolDef {
            name: "ping".to_string(),
            description: None,
            parameters: None,
        }];
        let calls = decode_calls("<<call ping {}>> <<call ping {}>>", &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "ping");
        assert_eq!(calls[1].name, "ping");
    }

    #[test]
    fn test_valid_call_followed_by_malformed_call_fails_closed() {
        let tools = sample_tools();
        let text = r#"
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Retrospective","start":"2023-11-02T10:00:00+05:30","duration_min":30,"visibility":"private"}}>>
"#;

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "malformed_call");
    }

    #[test]
    fn test_valid_call_followed_by_unknown_tool_fails_closed() {
        let tools = sample_tools();
        let text = r#"
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call delete_everything {}>>
"#;

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "unknown_tool");
    }

    #[test]
    fn test_valid_call_followed_by_invalid_arguments_fails_closed() {
        let tools = sample_tools();
        let text = r#"
<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
<<call create_calendar_event {"title":"Missing start"}>>
"#;

        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_all_malformed_calls_still_report_an_error() {
        let tools = sample_tools();
        let text = "<<call send_email {\"to\":[\"sam@example.com\"]}}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.error_code(), "malformed_call");
    }

    #[test]
    fn test_malformed_json_in_call() {
        let tools = vec![ToolDef {
            name: "foo".to_string(),
            description: None,
            parameters: None,
        }];
        let err = decode_calls("<<call foo {not json}>>", &tools).unwrap_err();
        assert_eq!(err.error_code(), "invalid_arguments");
    }

    #[test]
    fn test_unclosed_call_marker() {
        let tools = vec![ToolDef {
            name: "foo".to_string(),
            description: None,
            parameters: None,
        }];
        let err = decode_calls("<<call foo {\"a\":1}", &tools).unwrap_err();
        assert_eq!(err.error_code(), "malformed_call");
    }
}
