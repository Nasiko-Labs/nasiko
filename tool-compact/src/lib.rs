//! Compact, deterministic tool definitions and tool-call decoding.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;

const CALL_PREFIX: &str = "<<call ";
const CALL_SUFFIX: &str = ">>";

/// Tool definition owned by this crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
}

/// Decoded compact tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Encoded tool definitions ready for a model prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools(String);

impl CompactTools {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidToolName(String),
    InvalidSchema(String),
    UnsupportedSchema(String),
    UnknownTool(String),
    InvalidArguments(String),
    MalformedCall(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToolName(s) => write!(f, "invalid tool name: {s}"),
            Self::InvalidSchema(s) => write!(f, "invalid schema: {s}"),
            Self::UnsupportedSchema(s) => write!(f, "unsupported schema: {s}"),
            Self::UnknownTool(s) => write!(f, "unknown tool: {s}"),
            Self::InvalidArguments(s) => write!(f, "invalid arguments: {s}"),
            Self::MalformedCall(s) => write!(f, "malformed call: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Encode tool definitions into the compact prompt grammar.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    let mut out = String::new();

    for (index, tool) in tools.iter().enumerate() {
        validate_tool_name(&tool.name)?;
        let schema = normalize_schema(&tool.parameters)?;

        if index > 0 {
            out.push('\n');
        }

        out.push_str(&tool.name);
        out.push('(');
        write_properties(&mut out, &schema)?;
        out.push(')');

        if let Some(description) = &tool.description {
            out.push_str(" - ");
            out.push_str(description.trim());
        }
    }

    Ok(CompactTools(out))
}

/// Decode compact calls into structured tool calls.
pub fn decode_calls(input: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    let map: HashMap<&str, &ToolDef> = tools.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut decoder = StreamDecoder::new(tools);
    let mut calls = decoder.push(input)?;
    calls.extend(decoder.finish()?);

    for call in &calls {
        if !map.contains_key(call.name.as_str()) {
            return Err(Error::UnknownTool(call.name.clone()));
        }
    }

    Ok(calls)
}

/// Incremental decoder that supports stream chunks splitting the marker.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    cursor: usize,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            buffer: String::new(),
            cursor: 0,
        }
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, Error> {
        self.buffer.push_str(chunk);
        self.parse(false)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>, Error> {
        self.parse(true)
    }

    fn parse(&mut self, final_chunk: bool) -> Result<Vec<ToolCall>, Error> {
        let mut calls = Vec::new();

        loop {
            let Some(offset) = self.buffer[self.cursor..].find(CALL_PREFIX) else {
                if final_chunk {
                    if !self.buffer[self.cursor..].trim().is_empty() {
                        return Err(Error::MalformedCall("unexpected trailing text".into()));
                    }
                }
                break;
            };

            let start = self.cursor + offset;
            let body_start = start + CALL_PREFIX.len();

            let Some(end_offset) = self.buffer[body_start..].find(CALL_SUFFIX) else {
                if final_chunk {
                    return Err(Error::MalformedCall("incomplete call".into()));
                }
                break;
            };

            let end = body_start + end_offset;
            let body = &self.buffer[body_start..end];

            calls.push(parse_call_body(body, &self.tools)?);
            self.cursor = end + CALL_SUFFIX.len();
        }

        Ok(calls)
    }
}

fn validate_tool_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return Err(Error::InvalidToolName(name.to_string()));
    }

    Ok(())
}

fn normalize_schema(schema: &Value) -> Result<Value, Error> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::InvalidSchema("parameters must be an object".into()))?;

    if object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::UnsupportedSchema(
            "root parameters must be type object".into(),
        ));
    }

    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::InvalidSchema("parameters.properties must be an object".into()))?;

    for (name, property) in properties {
        validate_property(name, property)?;
    }

    if let Some(required) = object.get("required") {
        let Some(items) = required.as_array() else {
            return Err(Error::InvalidSchema("required must be an array".into()));
        };

        for item in items {
            if item.as_str().is_none() {
                return Err(Error::InvalidSchema(
                    "required entries must be strings".into(),
                ));
            }
        }
    }

    Ok(schema.clone())
}

fn validate_property(name: &str, schema: &Value) -> Result<(), Error> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::InvalidSchema(format!("property {name} must be an object")))?;

    let ty = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::UnsupportedSchema(format!("property {name} is missing type")))?;

    match ty {
        "string" | "number" | "integer" | "boolean" => {}

        "array" => {
            let items = object.get("items").ok_or_else(|| {
                Error::UnsupportedSchema(format!("array property {name} is missing items"))
            })?;

            validate_property(&format!("{name}[]"), items)?;
        }

        "object" => {
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    Error::UnsupportedSchema(format!(
                        "object property {name} is missing properties"
                    ))
                })?;

            for (child, child_schema) in properties {
                validate_property(&format!("{name}.{child}"), child_schema)?;
            }
        }

        other => {
            return Err(Error::UnsupportedSchema(format!(
                "unsupported type {other} for {name}"
            )));
        }
    }

    if let Some(en) = object.get("enum") {
        if !en.is_array() {
            return Err(Error::InvalidSchema(format!(
                "enum for {name} must be an array"
            )));
        }
    }

    Ok(())
}

fn write_properties(out: &mut String, schema: &Value) -> Result<(), Error> {
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| Error::InvalidSchema("parameters.properties must be an object".into()))?;

    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    for (index, (name, property)) in properties.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }

        out.push_str(name);

        if !required.contains(&name.as_str()) {
            out.push('?');
        }

        out.push(':');
        write_type(out, property)?;
    }

    Ok(())
}

fn write_type(out: &mut String, schema: &Value) -> Result<(), Error> {
    let ty = schema["type"]
        .as_str()
        .ok_or_else(|| Error::InvalidSchema("missing type".into()))?;

    match ty {
        "array" => {
            out.push('[');
            write_type(out, &schema["items"])?;
            out.push(']');
        }

        "object" => {
            out.push('{');
            write_properties(out, schema)?;
            out.push('}');
        }

        "string" | "number" | "integer" | "boolean" => {
            out.push_str(ty);
        }

        _ => {
            return Err(Error::UnsupportedSchema(ty.into()));
        }
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        out.push('|');

        for (index, value) in values.iter().enumerate() {
            if index > 0 {
                out.push('|');
            }

            let value = value
                .as_str()
                .ok_or_else(|| Error::UnsupportedSchema("enum values must be strings".into()))?;

            out.push_str(value);
        }
    }

    Ok(())
}

fn parse_call_body(body: &str, tools: &[ToolDef]) -> Result<ToolCall, Error> {
    let mut parts = body.splitn(2, char::is_whitespace);

    let name = parts.next().unwrap_or_default().trim();
    let args = parts.next().unwrap_or_default().trim();

    if name.is_empty() || args.is_empty() {
        return Err(Error::MalformedCall("expected <<call name {json}>>".into()));
    }

    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| Error::UnknownTool(name.into()))?;

    let arguments: Value =
        serde_json::from_str(args).map_err(|e| Error::InvalidArguments(e.to_string()))?;

    validate_arguments(&arguments, &tool.parameters).map_err(Error::InvalidArguments)?;

    Ok(ToolCall {
        name: name.into(),
        arguments,
    })
}

fn validate_arguments(arguments: &Value, schema: &Value) -> Result<(), String> {
    let args = arguments
        .as_object()
        .ok_or_else(|| "arguments must be a JSON object".to_string())?;

    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| "parameters.properties must be an object".to_string())?;

    let required = schema["required"].as_array().cloned().unwrap_or_default();

    for item in required {
        let name = item
            .as_str()
            .ok_or_else(|| "required entries must be strings".to_string())?;

        if !args.contains_key(name) {
            return Err(format!("missing required argument {name}"));
        }
    }

    for (name, value) in args {
        let property = properties
            .get(name)
            .ok_or_else(|| format!("unknown argument {name}"))?;

        validate_value(name, value, property)?;
    }

    Ok(())
}

fn validate_value(name: &str, value: &Value, schema: &Value) -> Result<(), String> {
    let ty = schema["type"]
        .as_str()
        .ok_or_else(|| format!("missing type for {name}"))?;

    let valid = match ty {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.as_array().is_some(),
        "object" => value.is_object(),
        _ => false,
    };

    if !valid {
        return Err(format!("invalid type for argument {name}"));
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.iter().any(|allowed| allowed == value) {
            return Err(format!("invalid enum value for argument {name}"));
        }
    }

    if ty == "array" {
        if let Some(items) = value.as_array() {
            for item in items {
                validate_value(&format!("{name}[]"), item, &schema["items"])?;
            }
        }
    }

    if ty == "object" {
        validate_arguments(value, schema)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event".into()),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string"},
                    "duration_min": {"type": "integer"},
                    "attendees": {
                        "type": "array",
                        "items": {"type": "string"}
                    },
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title", "start"]
            }),
        }]
    }

    #[test]
    fn encodes_required_optional_types_and_enums() {
        let encoded = encode_tools(&tools()).unwrap().into_string();

        assert_eq!(
            encoded,
            "create_calendar_event(title:string, start:string, \
duration_min?:integer, attendees?:[string], \
visibility?:string|public|private) - Create an event"
        );
    }

    #[test]
    fn decodes_valid_call() {
        let calls = decode_calls(
            r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-02T10:00:00+05:30","visibility":"private"}>>"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
    }

    #[test]
    fn unknown_tool_fails_closed() {
        assert!(matches!(
            decode_calls(r#"<<call missing {"x":1}>>"#, &tools()),
            Err(Error::UnknownTool(_))
        ));
    }

    #[test]
    fn missing_required_argument_fails_closed() {
        assert!(matches!(
            decode_calls(
                r#"<<call create_calendar_event {"visibility":"private"}>>"#,
                &tools()
            ),
            Err(Error::InvalidArguments(_))
        ));
    }

    #[test]
    fn invalid_enum_fails_closed() {
        assert!(matches!(
            decode_calls(
                r#"<<call create_calendar_event {"title":"x","start":"y","visibility":"team"}>>"#,
                &tools()
            ),
            Err(Error::InvalidArguments(_))
        ));
    }

    #[test]
    fn split_marker_across_stream_chunks_is_supported() {
        let mut decoder = StreamDecoder::new(&tools());

        assert!(decoder.push("<<ca").unwrap().is_empty());

        let calls = decoder
            .push(r#"ll create_calendar_event {"title":"x","start":"y"}>>"#)
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert!(decoder.finish().unwrap().is_empty());
    }

    #[test]
    fn malformed_call_fails_closed() {
        assert!(
            decode_calls(
                r#"<<call create_calendar_event {"title":"x","start":"y"}"#,
                &tools()
            )
            .is_err()
        );
    }

    #[test]
    fn nested_objects_and_arrays_are_validated() {
        let nested = ToolDef {
            name: "send".into(),
            description: None,
            parameters: json!({
                "type": "object",
                "properties": {
                    "payload": {
                        "type": "object",
                        "properties": {
                            "ids": {
                                "type": "array",
                                "items": {"type": "integer"}
                            }
                        },
                        "required": ["ids"]
                    }
                },
                "required": ["payload"]
            }),
        };

        let calls =
            decode_calls(r#"<<call send {"payload":{"ids":[1,2,3]}}>>"#, &[nested]).unwrap();

        assert_eq!(calls[0].name, "send");
    }
}
