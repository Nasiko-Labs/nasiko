use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

static CALL_START_REGEX: OnceLock<Regex> = OnceLock::new();

fn get_call_start_regex() -> &'static Regex {
    CALL_START_REGEX.get_or_init(|| {
        Regex::new(r"<<call\s+([a-zA-Z0-9_\-]+)\s+").expect("invalid call regex")
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

/// Encodes JSON schema tools into compact signature lines[cite: 4]
/// Places required fields first, followed by optional fields[cite: 4]
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
            // 1. Required fields first (in defined order)
            for req_name in &required_fields {
                if let Some(schema) = props.get(req_name) {
                    let ty = format_type(schema);
                    param_sigs.push(format!("{}:{}", req_name, ty));
                }
            }

            // 2. Optional fields next
            for (name, schema) in props {
                if !required_fields.contains(name) {
                    let ty = format_type(schema);
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

/// Parses balanced JSON and extracts call markers, handling `>>` inside string args[cite: 4]
fn extract_next_call(text: &str) -> Option<(usize, usize, String, String)> {
    let re = get_call_start_regex();
    let marker_idx = text.find("<<call")?;
    let remainder = &text[marker_idx..];
    let mat = re.find(remainder)?;
    let caps = re.captures(remainder)?;
    let tool_name = caps.get(1)?.as_str().to_string();

    let json_start = marker_idx + mat.end();
    if json_start >= text.len() || !text[json_start..].starts_with('{') {
        return None;
    }

    let mut in_str = false;
    let mut is_escaped = false;
    let mut brace_depth = 0;
    let mut json_end = None;

    for (rel_i, c) in text[json_start..].char_indices() {
        if is_escaped {
            is_escaped = false;
            continue;
        }
        if c == '\\' {
            is_escaped = true;
            continue;
        }
        if c == '"' {
            in_str = !in_str;
            continue;
        }
        if !in_str {
            if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth -= 1;
                if brace_depth == 0 {
                    json_end = Some(json_start + rel_i + 1);
                    break;
                }
            }
        }
    }

    let j_end = json_end?;
    let tail = &text[j_end..];
    let after_ws = tail.trim_start();
    if after_ws.starts_with(">>") {
        let ws_len = tail.len() - after_ws.len();
        let full_end = j_end + ws_len + 2;
        let json_str = text[json_start..j_end].to_string();
        Some((marker_idx, full_end, tool_name, json_str))
    } else {
        None
    }
}

/// Decodes non-streaming compact tool calls[cite: 4]
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut calls = Vec::new();
    let mut cursor = 0;

    while cursor < text.len() {
        if let Some((_, end, name, raw_args)) = extract_next_call(&text[cursor..]) {
            let args: Value = serde_json::from_str(&raw_args)
                .map_err(|e| CompactError::InvalidArguments(e.to_string()))?;

            let call = ToolCall {
                name,
                arguments: args,
            };
            validate_call(&call, tools)?;
            calls.push(call);
            cursor += end;
        } else {
            break;
        }
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

        while let Some((start, end, name, raw_args)) = extract_next_call(&self.buffer) {
            let args: Value = serde_json::from_str(&raw_args)
                .map_err(|e| CompactError::InvalidArguments(e.to_string()))?;

            let call = ToolCall {
                name,
                arguments: args,
            };
            validate_call(&call, self.tools)?;
            calls.push(call);
            self.buffer.drain(..end);
            let _ = start;
        }
        Ok(calls)
    }

    pub fn finish(self) -> Result<Vec<ToolCall>, CompactError> {
        let calls = decode_calls(&self.buffer, self.tools)?;
        Ok(calls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create a calendar event".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "send_email".into(),
                description: Some("Send an email".into()),
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
    fn test_encoding_compact_format() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.prompt_injection.contains("create_calendar_event("));
        assert!(compact.prompt_injection.contains("title:str, start:datetime"));
        assert!(compact.prompt_injection.contains("duration_min?:int"));
        assert!(compact.prompt_injection.contains("visibility?:public|private"));
    }

    #[test]
    fn test_stream_decoder_split_chunks() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(&tools);

        let chunks = vec![
            "<<ca",
            "ll create_calendar_event {\"title\":\"Design Review\",",
            "\"start\":\"2026-10-05T15:00:00+05:30\"}>",
            ">",
        ];

        let mut collected = Vec::new();
        for chunk in chunks {
            collected.extend(decoder.push_chunk(chunk).unwrap());
        }
        collected.extend(decoder.finish().unwrap());

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].name, "create_calendar_event");
        assert_eq!(collected[0].arguments["title"], "Design Review");
    }

    #[test]
    fn test_escaped_arrow_in_string_argument() {
        let tools = sample_tools();
        let input = r#"<<call send_email {"to":["a@b.com"],"subject":"Test","body":"Check this >> arrow"}>>"#;
        let res = decode_calls(input, &tools).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].arguments["body"], "Check this >> arrow");
    }

    #[test]
    fn test_fail_closed_unknown_tool() {
        let tools = sample_tools();
        let input = "<<call unknown_service {\"foo\":\"bar\"}>>";
        let res = decode_calls(input, &tools);
        assert!(matches!(res, Err(CompactError::UnknownTool(_))));
    }

    #[test]
    fn test_fail_closed_missing_required() {
        let tools = sample_tools();
        let input = "<<call create_calendar_event {\"title\":\"Quick Sync\"}>>";
        let res = decode_calls(input, &tools);
        assert!(matches!(res, Err(CompactError::InvalidArguments(_))));
    }

    #[test]
    fn test_fail_closed_enum_violation() {
        let tools = sample_tools();
        let input = "<<call create_calendar_event {\"title\":\"Sync\",\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>";
        let res = decode_calls(input, &tools);
        assert!(matches!(res, Err(CompactError::InvalidArguments(_))));
    }
}