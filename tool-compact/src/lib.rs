//! Compact tool schemas and fail-closed decoding for LLM tool calls.
//!
//! Grammar:
//!   call := "<<call " tool-name " " json-object ">>"
//!   tool-name := [A-Za-z0-9_.:-]+
//!
//! JSON is used for arguments, so normal JSON escaping protects marker-like
//! text such as `>>` inside strings. Text before/between/after calls is allowed.
//! An incomplete marker is buffered by `StreamDecoder`; malformed completed
//! calls fail closed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

const OPEN: &str = "<<call ";
const CLOSE: &str = ">>";
const MAX_DESCRIPTION: usize = 96;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("unsupported_schema: {0}")]
    UnsupportedSchema(String),
    #[error("invalid_schema: {0}")]
    InvalidSchema(String),
    #[error("unknown_tool: {0}")]
    UnknownTool(String),
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    #[error("malformed_call: {0}")]
    MalformedCall(String),
    #[error("incomplete_call")]
    IncompleteCall,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTool {
    pub name: String,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    pub tools: Vec<CompactTool>,
    pub instructions: String,
}

impl CompactTools {
    pub fn prompt(&self) -> &str {
        &self.instructions
    }
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut seen = BTreeMap::new();
    let mut compact = Vec::with_capacity(tools.len());

    for tool in tools {
        if !is_valid_name(&tool.name) {
            return Err(Error::InvalidSchema(format!(
                "tool name {:?} is not valid",
                tool.name
            )));
        }
        if seen.insert(tool.name.clone(), ()).is_some() {
            return Err(Error::InvalidSchema(format!(
                "duplicate tool {:?}",
                tool.name
            )));
        }

        let schema = tool.parameters.as_ref().unwrap_or(&Value::Null);
        let signature = if schema.is_null() {
            "()".to_string()
        } else {
            compact_schema(schema, "$")?
        };

        compact.push(CompactTool {
            name: tool.name.clone(),
            signature,
            description: tool.description.as_deref().map(short_description),
            parameters: tool.parameters.clone(),
        });
    }

    let mut lines = Vec::with_capacity(compact.len() + 2);
    lines.push("Available tools:".to_string());
    for t in &compact {
        let desc = t
            .description
            .as_deref()
            .map(|d| format!(" - {d}"))
            .unwrap_or_default();
        lines.push(format!("{}{}{}", t.name, t.signature, desc));
    }
    lines.push(
        "To call a tool, emit exactly <<call name {json args}>>. You may emit multiple calls. \
         JSON arguments must satisfy the declared schema. Do not invent tools or fields."
            .to_string(),
    );

    Ok(CompactTools {
        tools: compact,
        instructions: lines.join("\n"),
    })
}

/// Reconstruct the compact representation's source definitions. This is useful
/// for deterministic schema-preservation checks in the evaluation harness.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    Ok(compact
        .tools
        .iter()
        .map(|t| ToolDef {
            name: t.name.clone(),
            description: t.description.clone(),
            parameters: t.parameters.clone(),
        })
        .collect())
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut cursor = 0usize;
    let mut calls = Vec::new();

    while let Some(relative) = text[cursor..].find(OPEN) {
        let start = cursor + relative;
        let after_open = start + OPEN.len();
        let (call, consumed) = parse_one(&text[after_open..], tools)?;
        calls.push(call);
        cursor = after_open + consumed;
    }

    // A marker-looking prefix without a complete call is never silently ignored.
    if let Some(relative) = text[cursor..].find("<<call") {
        let tail = &text[cursor + relative..];
        if !tail.is_empty() {
            return Err(Error::MalformedCall(
                "found a call marker that does not match the grammar".into(),
            ));
        }
    }

    Ok(calls)
}

fn parse_one(input: &str, tools: &[ToolDef]) -> Result<(ToolCall, usize)> {
    let name_end = input
        .find(char::is_whitespace)
        .ok_or_else(|| Error::IncompleteCall)?;
    let name = &input[..name_end];

    if !is_valid_name(name) {
        return Err(Error::MalformedCall(format!("invalid tool name {name:?}")));
    }

    let json_start = name_end
        + input[name_end..]
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
    let json_text = &input[json_start..];

    let mut stream = serde_json::Deserializer::from_str(json_text).into_iter::<Value>();
    let value = match stream.next() {
        Some(Ok(v)) => v,
        Some(Err(e)) if e.is_eof() => return Err(Error::IncompleteCall),
        Some(Err(e)) => return Err(Error::MalformedCall(format!("invalid JSON arguments: {e}"))),
        None => return Err(Error::IncompleteCall),
    };
    let offset = stream.byte_offset();
    let trailing = &json_text[offset..];

    if !trailing.starts_with(CLOSE) {
        if trailing.trim_start().is_empty() || CLOSE.starts_with(trailing.trim_start()) {
            return Err(Error::IncompleteCall);
        }
        return Err(Error::MalformedCall(
            "JSON arguments must be followed immediately by >>".into(),
        ));
    }

    let consumed = json_start + offset + CLOSE.len();
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

    validate_arguments(&value, tool)?;
    Ok((
        ToolCall {
            name: name.to_string(),
            arguments: value,
        },
        consumed,
    ))
}

fn validate_arguments(args: &Value, tool: &ToolDef) -> Result<()> {
    let schema = tool.parameters.as_ref();
    if schema.is_none() || schema.is_some_and(Value::is_null) {
        if args.as_object().is_some_and(|m| m.is_empty()) {
            return Ok(());
        }
        return Err(Error::InvalidArguments(format!(
            "{} does not accept arguments",
            tool.name
        )));
    }
    validate_value(args, schema.unwrap(), "$")
}

fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<()> {
    let obj = schema
        .as_object()
        .ok_or_else(|| Error::InvalidSchema(format!("{path}: schema must be an object")))?;

    for key in [
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "dependentRequired",
        "dependentSchemas",
    ] {
        if obj.contains_key(key) {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: {key} is unsupported"
            )));
        }
    }

    if let Some(enum_values) = obj.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or_else(|| Error::InvalidSchema(format!("{path}: enum must be an array")))?;
        if !values.iter().any(|candidate| candidate == value) {
            return Err(Error::InvalidArguments(format!(
                "{path}: value is not in enum"
            )));
        }
    }

    if let Some(expected) = obj.get("type") {
        let expected = expected
            .as_str()
            .ok_or_else(|| Error::UnsupportedSchema(format!("{path}: type must be a string")))?;
        if !type_matches(value, expected) {
            return Err(Error::InvalidArguments(format!(
                "{path}: expected {expected}"
            )));
        }
    }

    match obj.get("type").and_then(Value::as_str) {
        Some("object") => {
            let map = value
                .as_object()
                .ok_or_else(|| Error::InvalidArguments(format!("{path}: expected object")))?;

            let properties = obj
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    Error::InvalidSchema(format!("{path}: object requires properties"))
                })?;

            let required: &[Value] = match obj.get("required") {
                None => &[],
                Some(v) => v.as_array().ok_or_else(|| {
                    Error::InvalidSchema(format!("{path}: required must be an array"))
                })?,
            };

            for req in required {
                let name = req.as_str().ok_or_else(|| {
                    Error::InvalidSchema(format!("{path}: required entries must be strings"))
                })?;
                if !map.contains_key(name) {
                    return Err(Error::InvalidArguments(format!(
                        "{path}.{name}: required field is missing"
                    )));
                }
            }

            // Fail closed on undeclared properties. The compact grammar has no
            // safe way to represent arbitrary additional fields.
            for key in map.keys() {
                if !properties.contains_key(key) {
                    return Err(Error::InvalidArguments(format!(
                        "{path}.{key}: unknown field"
                    )));
                }
            }

            for (key, child) in properties {
                if let Some(v) = map.get(key) {
                    validate_value(v, child, &format!("{path}.{key}"))?;
                }
            }
        }
        Some("array") => {
            let items = obj
                .get("items")
                .ok_or_else(|| Error::InvalidSchema(format!("{path}: array requires items")))?;
            let array = value
                .as_array()
                .ok_or_else(|| Error::InvalidArguments(format!("{path}: expected array")))?;
            for (i, item) in array.iter().enumerate() {
                validate_value(item, items, &format!("{path}[{i}]"))?;
            }
        }
        Some("string") => {
            if let Some(format) = obj.get("format").and_then(Value::as_str) {
                // Formats are advisory to JSON Schema, but unsupported semantics
                // must not be silently dropped. Preserve common descriptive formats.
                if !matches!(
                    format,
                    "date-time" | "date" | "time" | "email" | "uri" | "url"
                ) {
                    return Err(Error::UnsupportedSchema(format!(
                        "{path}: format {format:?} is unsupported"
                    )));
                }
            }
        }
        Some("integer") | Some("number") | Some("boolean") | None => {}
        Some(other) => {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: type {other:?} is unsupported"
            )));
        }
    }

    for keyword in ["minLength", "maxLength", "minimum", "maximum", "pattern"] {
        if obj.contains_key(keyword) {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: constraint {keyword} is unsupported"
            )));
        }
    }

    if let Some(additional) = obj.get("additionalProperties") {
        if !additional.is_boolean() {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: schema-valued additionalProperties is unsupported"
            )));
        }
        if additional.as_bool() == Some(true) {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: additionalProperties=true is unsupported"
            )));
        }
    }

    Ok(())
}

fn type_matches(value: &Value, expected: &str) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

fn compact_schema(schema: &Value, path: &str) -> Result<String> {
    let obj = schema
        .as_object()
        .ok_or_else(|| Error::InvalidSchema(format!("{path}: schema must be an object")))?;

    for key in [
        "oneOf",
        "anyOf",
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "dependentRequired",
        "dependentSchemas",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "pattern",
        "minItems",
        "maxItems",
        "uniqueItems",
    ] {
        if obj.contains_key(key) {
            return Err(Error::UnsupportedSchema(format!(
                "{path}: {key} is unsupported"
            )));
        }
    }

    if let Some(additional) = obj.get("additionalProperties") {
        match additional {
            Value::Bool(false) => {}
            Value::Bool(true) => {
                return Err(Error::UnsupportedSchema(format!(
                    "{path}: additionalProperties=true is unsupported"
                )));
            }
            _ => {
                return Err(Error::UnsupportedSchema(format!(
                    "{path}: schema-valued additionalProperties is unsupported"
                )));
            }
        }
    }

    if let Some(enum_values) = obj.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or_else(|| Error::InvalidSchema(format!("{path}: enum must be an array")))?;
        let rendered = values
            .iter()
            .map(render_enum)
            .collect::<Result<Vec<_>>>()?
            .join("|");
        return Ok(rendered);
    }

    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::UnsupportedSchema(format!("{path}: missing string type")))?;

    match ty {
        "object" => {
            let props = obj
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    Error::InvalidSchema(format!("{path}: object requires properties"))
                })?;
            let required: &[Value] = match obj.get("required") {
                None => &[],
                Some(v) => v.as_array().ok_or_else(|| {
                    Error::InvalidSchema(format!("{path}: required must be an array"))
                })?,
            };

            let mut parts = Vec::with_capacity(props.len());
            for (name, prop) in props {
                let rendered = compact_schema(prop, &format!("{path}.{name}"))?;
                let optional = !required.iter().any(|r| r.as_str() == Some(name));

                let mut field = if optional {
                    format!("{name}?:{rendered}")
                } else {
                    format!("{name}:{rendered}")
                };

                if let Some(desc) = prop.get("description").and_then(Value::as_str) {
                    let desc = short_description(desc);
                    if !desc.is_empty() {
                        field.push_str("=");
                        field.push_str(&desc);
                    }
                }

                parts.push(field);
            }
            let mut out = String::from("(");
            out.push_str(&parts.join(","));
            out.push(')');
            Ok(out)
        }
        "array" => {
            let items = obj
                .get("items")
                .ok_or_else(|| Error::InvalidSchema(format!("{path}: array requires items")))?;
            Ok(format!(
                "[{}]",
                compact_schema(items, &format!("{path}[]"))?
            ))
        }
        "string" => {
            if let Some(format) = obj.get("format").and_then(Value::as_str) {
                return Ok(match format {
                    "date-time" => "datetime".into(),
                    "date" => "date".into(),
                    "time" => "time".into(),
                    "email" => "email".into(),
                    "uri" | "url" => "url".into(),
                    other => {
                        return Err(Error::UnsupportedSchema(format!(
                            "{path}: format {other:?} is unsupported"
                        )));
                    }
                });
            }
            Ok("str".into())
        }
        "integer" => Ok("int".into()),
        "number" => Ok("num".into()),
        "boolean" => Ok("bool".into()),
        other => Err(Error::UnsupportedSchema(format!(
            "{path}: type {other:?} is unsupported"
        ))),
    }
}

fn render_enum(value: &Value) -> Result<String> {
    match value {
        Value::String(s) if !s.contains(['|', '>', '<', '\n', '\r']) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".into()),
        _ => Err(Error::UnsupportedSchema(
            "enum values must be scalar JSON values".into(),
        )),
    }
}

fn short_description(s: &str) -> String {
    let mut out = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.len() > MAX_DESCRIPTION {
        out.truncate(MAX_DESCRIPTION);
        out.push_str("...");
    }
    out
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
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

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();

        loop {
            let Some(start) = self.buffer.find(OPEN) else {
                let keep = OPEN.len().saturating_sub(1);
                if self.buffer.len() > keep {
                    let drain = self.buffer.len() - keep;
                    self.buffer.drain(..drain);
                }
                break;
            };

            if start > 0 {
                self.buffer.drain(..start);
            }

            match parse_one(&self.buffer[OPEN.len()..], &self.tools) {
                Ok((call, consumed)) => {
                    self.buffer.drain(..OPEN.len() + consumed);
                    out.push(call);
                }
                Err(Error::IncompleteCall) => break,
                Err(e) => return Err(e),
            }
        }

        Ok(out)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        if self.buffer.contains(OPEN) {
            return decode_calls(&self.buffer, &self.tools);
        }
        if self.buffer.contains("<<call") {
            return Err(Error::MalformedCall(
                "incomplete or malformed call marker".into(),
            ));
        }
        self.buffer.clear();
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event.".into()),
            parameters: Some(json!({
                "type":"object",
                "properties":{
                    "title":{"type":"string","description":"Event title"},
                    "start":{"type":"string","format":"date-time"},
                    "duration_min":{"type":"integer"},
                    "attendees":{"type":"array","items":{"type":"string"}},
                    "visibility":{"type":"string","enum":["public","private"]}
                },
                "required":["title","start"],
                "additionalProperties":false
            })),
        }]
    }

    #[test]
    fn compact_format_keeps_required_optional_enum_nested_and_array_semantics() {
        let encoded = encode_tools(&tools()).unwrap();
        assert!(encoded.instructions.contains("title:str"));
        assert!(encoded.instructions.contains("duration_min?:int"));
        assert!(encoded.instructions.contains("attendees?:[str]"));
        assert!(encoded.instructions.contains("visibility?:public|private"));
        assert!(encoded.instructions.contains("start:datetime"));
    }

    #[test]
    fn round_trip_valid_call() {
        let text = r#">>noise<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","visibility":"private"}>>done"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["visibility"], "private");
    }

    #[test]
    fn multiple_calls_and_plain_text_are_supported() {
        let text = r#"before <<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00+05:30"}>> middle <<call create_calendar_event {"title":"B","start":"2026-10-06T15:00:00+05:30"}>> after"#;
        assert_eq!(decode_calls(text, &tools()).unwrap().len(), 2);
        assert!(
            decode_calls("plain answer only", &tools())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let err = decode_calls(r#"<<call delete_everything {}>>"#, &tools()).unwrap_err();
        assert!(matches!(err, Error::UnknownTool(_)));
    }

    #[test]
    fn missing_required_fails_closed() {
        let err = decode_calls(
            r#"<<call create_calendar_event {"visibility":"private"}>>"#,
            &tools(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));
    }

    #[test]
    fn invalid_enum_and_type_fail_closed() {
        let err = decode_calls(
            r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            &tools(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));

        let err = decode_calls(
            r#"<<call create_calendar_event {"title":12,"start":"2026-10-05T15:00:00+05:30"}>>"#,
            &tools(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments(_)));
    }

    #[test]
    fn marker_inside_json_string_is_safe() {
        let text = r#"<<call create_calendar_event {"title":"say >> literally","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = decode_calls(text, &tools()).unwrap();
        assert_eq!(calls[0].arguments["title"], "say >> literally");
    }

    #[test]
    fn streaming_handles_split_marker_and_json() {
        let mut d = StreamDecoder::new(tools());
        assert!(d.push("prefix <<ca").unwrap().is_empty());
        assert!(
            d.push("ll create_calendar_event {\"title\":\"Ret")
                .unwrap()
                .is_empty()
        );
        assert!(
            d.push("ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>")
                .unwrap()
                .is_empty()
        );
        let calls = d.push(">").unwrap();
        assert_eq!(calls[0].arguments["title"], "Retro");
        assert!(d.finish().unwrap().is_empty());
    }

    #[test]
    fn unsupported_schema_is_explicit() {
        let tool = ToolDef {
            name: "x".into(),
            description: None,
            parameters: Some(json!({
                "type":"object",
                "properties":{"x":{"type":"string","pattern":"^a+$"}},
                "required":["x"]
            })),
        };
        assert!(matches!(
            encode_tools(&[tool]),
            Err(Error::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn malformed_call_is_not_silently_ignored() {
        let err = decode_calls("<<call create_calendar_event {bad}>>", &tools()).unwrap_err();
        assert!(matches!(err, Error::MalformedCall(_)));
    }
}
