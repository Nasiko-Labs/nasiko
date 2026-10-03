//! Compact text tool definitions with strict decoding against the original JSON Schema.
//!
//! The wire grammar is `<<call NAME {"argument":"value"}>>`. JSON is scanned
//! lexically before the closing marker is considered, so marker-like text in a JSON
//! string never terminates a call.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

const MARKER: &str = "<<call";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
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

#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    /// Text placed in a system message instead of the provider-native `tools` array.
    pub prompt: String,
    tools: Vec<ToolDef>,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported schema for tool `{tool}`: {reason}")]
    UnsupportedSchema { tool: String, reason: String },
    #[error("malformed compact tool call: {0}")]
    MalformedCall(String),
    #[error("unknown tool `{0}")]
    UnknownTool(String),
    #[error("invalid arguments for tool `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },
    #[error("invalid JSON arguments: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn function_kind() -> String {
    "function".into()
}

/// Convert supported native tool schemas into a compact instruction block.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = vec![
        "Call only: <<call NAME JSON>>".to_string(),
        "?: optional; [T]: array; !: no extra keys; dt: ISO-8601 datetime.".to_string(),
    ];

    let mut names = BTreeSet::new();
    for tool in tools {
        ensure_tool_supported(tool)?;
        if !names.insert(&tool.function.name) {
            return Err(Error::UnsupportedSchema {
                tool: tool.function.name.clone(),
                reason: "duplicate tool name".into(),
            });
        }
        let parameters = match &tool.function.parameters {
            Some(schema) => render_root_schema(schema, &tool.function.name)?,
            None => String::new(),
        };
        let description = tool
            .function
            .description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| format!(" - {text}"))
            .unwrap_or_default();
        lines.push(format!("{}({parameters}){description}", tool.function.name));
    }

    Ok(CompactTools {
        prompt: lines.join("\n"),
        tools: tools.to_vec(),
    })
}

/// Recover the original supported schemas. This intentionally returns the canonical
/// definitions retained by `CompactTools`, rather than attempting to infer JSON Schema
/// from a lossy display string.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    for tool in &compact.tools {
        ensure_tool_supported(tool)?;
    }
    Ok(compact.tools.clone())
}

/// Render structured calls in this crate's grammar. Useful for deterministic evaluation.
pub fn render_calls(calls: &[ToolCall]) -> Result<String> {
    calls
        .iter()
        .map(|call| {
            if !call.arguments.is_object() {
                return Err(Error::InvalidArguments {
                    tool: call.name.clone(),
                    reason: "tool arguments must be a JSON object".into(),
                });
            }
            Ok(format!(
                "<<call {} {}>>",
                call.name,
                serde_json::to_string(&call.arguments).expect("Value serialization cannot fail")
            ))
        })
        .collect::<Result<Vec<_>>>()
        .map(|calls| calls.join("\n"))
}

/// Decode all complete calls in a model response.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}

/// Incremental compact-call decoder. `push` returns calls completed by that chunk;
/// `finish` reports an incomplete call as an error.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    pending: String,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        for tool in tools {
            ensure_tool_supported(tool)?;
        }
        Ok(Self {
            tools: tools.to_vec(),
            pending: String::new(),
        })
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.pending.push_str(chunk);
        let mut calls = Vec::new();

        loop {
            let Some(marker_at) = self.pending.find(MARKER) else {
                retain_marker_prefix(&mut self.pending);
                return Ok(calls);
            };
            if marker_at > 0 {
                self.pending.drain(..marker_at);
            }

            match parse_call_prefix(&self.pending, &self.tools)? {
                Parsed::Complete { call, consumed } => {
                    self.pending.drain(..consumed);
                    calls.push(call);
                }
                Parsed::Incomplete => return Ok(calls),
            }
        }
    }

    pub fn finish(mut self) -> Result<Vec<ToolCall>> {
        let calls = self.push("")?;
        if self.pending.starts_with(MARKER) {
            return Err(Error::MalformedCall(
                "stream ended before the call was complete".into(),
            ));
        }
        Ok(calls)
    }
}

enum Parsed {
    Complete { call: ToolCall, consumed: usize },
    Incomplete,
}

fn parse_call_prefix(input: &str, tools: &[ToolDef]) -> Result<Parsed> {
    debug_assert!(input.starts_with(MARKER));
    let bytes = input.as_bytes();
    let mut pos = MARKER.len();
    if pos == bytes.len() {
        return Ok(Parsed::Incomplete);
    }
    if !bytes[pos].is_ascii_whitespace() {
        return Err(Error::MalformedCall(
            "expected whitespace after <<call".into(),
        ));
    }
    pos = skip_ws(bytes, pos);
    if pos == bytes.len() {
        return Ok(Parsed::Incomplete);
    }
    let name_start = pos;
    while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if name_start == pos {
        return Err(Error::MalformedCall("missing tool name".into()));
    }
    if pos == bytes.len() {
        return Ok(Parsed::Incomplete);
    }
    let name = &input[name_start..pos];
    pos = skip_ws(bytes, pos);
    if pos == bytes.len() {
        return Ok(Parsed::Incomplete);
    }
    if bytes[pos] != b'{' {
        return Err(Error::MalformedCall(
            "arguments must start with a JSON object".into(),
        ));
    }
    let Some(json_end) = end_of_json_object(bytes, pos)? else {
        return Ok(Parsed::Incomplete);
    };
    let mut end = skip_ws(bytes, json_end);
    if end + 2 > bytes.len() {
        return Ok(Parsed::Incomplete);
    }
    if &input[end..end + 2] != ">>" {
        return Err(Error::MalformedCall("expected >> after JSON object".into()));
    }
    end += 2;
    let arguments: Value = serde_json::from_str(&input[pos..json_end])?;
    let tool = tools
        .iter()
        .find(|tool| tool.function.name == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;
    validate_arguments(tool, &arguments)?;
    Ok(Parsed::Complete {
        call: ToolCall {
            name: name.to_string(),
            arguments,
        },
        consumed: end,
    })
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

fn end_of_json_object(bytes: &[u8], start: usize) -> Result<Option<usize>> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (pos, byte) in bytes.iter().copied().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                if depth == 0 {
                    return Err(Error::MalformedCall("unexpected } in arguments".into()));
                }
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(pos + 1));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

fn retain_marker_prefix(text: &mut String) {
    let keep = (1..MARKER.len())
        .rev()
        .find(|&length| text.ends_with(&MARKER[..length]))
        .unwrap_or(0);
    if keep == 0 {
        text.clear();
    } else {
        text.drain(..text.len() - keep);
    }
}

fn ensure_tool_supported(tool: &ToolDef) -> Result<()> {
    if tool.kind != "function" {
        return Err(Error::UnsupportedSchema {
            tool: tool.function.name.clone(),
            reason: "only function tools are supported".into(),
        });
    }
    if tool.function.name.is_empty() || tool.function.name.chars().any(char::is_whitespace) {
        return Err(Error::UnsupportedSchema {
            tool: tool.function.name.clone(),
            reason: "tool names must be non-empty and contain no whitespace".into(),
        });
    }
    if let Some(schema) = &tool.function.parameters {
        validate_schema_shape(schema, &tool.function.name)?;
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(Error::UnsupportedSchema {
                tool: tool.function.name.clone(),
                reason: "function parameters must be an object schema".into(),
            });
        }
    }
    Ok(())
}

fn validate_schema_shape(schema: &Value, tool: &str) -> Result<()> {
    let object = schema.as_object().ok_or_else(|| Error::UnsupportedSchema {
        tool: tool.into(),
        reason: "schema must be an object".into(),
    })?;
    let allowed: BTreeSet<&str> = [
        "type",
        "properties",
        "required",
        "items",
        "enum",
        "description",
        "format",
        "additionalProperties",
    ]
    .into_iter()
    .collect();
    if let Some(key) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(Error::UnsupportedSchema {
            tool: tool.into(),
            reason: format!("keyword `{key}` is not supported"),
        });
    }
    if let Some(kind) = object.get("type") {
        match kind {
            Value::String(kind) if is_supported_type(kind) => {}
            Value::Array(kinds)
                if !kinds.is_empty()
                    && kinds
                        .iter()
                        .all(|kind| kind.as_str().is_some_and(is_supported_type)) => {}
            _ => {
                return Err(Error::UnsupportedSchema {
                    tool: tool.into(),
                    reason: "type must be a supported type or non-empty type array".into(),
                });
            }
        }
    }
    if let Some(properties) = object.get("properties") {
        let properties = properties
            .as_object()
            .ok_or_else(|| Error::UnsupportedSchema {
                tool: tool.into(),
                reason: "properties must be an object".into(),
            })?;
        for child in properties.values() {
            validate_schema_shape(child, tool)?;
        }
    }
    if let Some(required) = object.get("required") {
        if !required
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err(Error::UnsupportedSchema {
                tool: tool.into(),
                reason: "required must be an array of strings".into(),
            });
        }
    }
    if let Some(items) = object.get("items") {
        validate_schema_shape(items, tool)?;
    }
    if let Some(values) = object.get("enum") {
        if !values.as_array().is_some_and(|values| !values.is_empty()) {
            return Err(Error::UnsupportedSchema {
                tool: tool.into(),
                reason: "enum must be a non-empty array".into(),
            });
        }
    }
    if let Some(value) = object.get("description")
        && !value.is_string()
    {
        return Err(Error::UnsupportedSchema {
            tool: tool.into(),
            reason: "description must be a string".into(),
        });
    }
    if let Some(value) = object.get("format")
        && !value.is_string()
    {
        return Err(Error::UnsupportedSchema {
            tool: tool.into(),
            reason: "format must be a string".into(),
        });
    }
    if let Some(value) = object.get("additionalProperties")
        && !value.is_boolean()
    {
        return Err(Error::UnsupportedSchema {
            tool: tool.into(),
            reason: "additionalProperties must be boolean".into(),
        });
    }
    Ok(())
}

fn is_supported_type(kind: &str) -> bool {
    matches!(
        kind,
        "object" | "array" | "string" | "integer" | "number" | "boolean" | "null"
    )
}

fn render_root_schema(schema: &Value, tool: &str) -> Result<String> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = required_names(schema, tool)?;
    properties
        .iter()
        .map(|(name, schema)| {
            let optional = (!required.contains(name)).then_some("?").unwrap_or("");
            Ok(render_field(name, optional, schema, tool)?)
        })
        .collect::<Result<Vec<_>>>()
        .map(|fields| fields.join(","))
}

fn render_schema(schema: &Value, tool: &str) -> Result<String> {
    let object = schema.as_object().expect("validated schema must be object");
    if let Some(values) = object.get("enum") {
        return Ok(render_enum(values));
    }
    let kinds: Vec<&str> = match object.get("type") {
        Some(Value::String(kind)) => vec![kind],
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        None if object.contains_key("properties") => vec!["object"],
        None => vec!["any"],
        _ => unreachable!("validated type"),
    };
    let rendered = kinds
        .iter()
        .map(|kind| -> Result<String> {
            Ok(match *kind {
                "string" => object
                    .get("format")
                    .and_then(Value::as_str)
                    .map(|format| match format {
                        "date-time" => "dt".into(),
                        _ => format!("str<{format}>"),
                    })
                    .unwrap_or_else(|| "str".into()),
                "integer" => "int".into(),
                "number" => "num".into(),
                "boolean" => "bool".into(),
                "null" => "null".into(),
                "array" => {
                    let items = object
                        .get("items")
                        .ok_or_else(|| Error::UnsupportedSchema {
                            tool: tool.into(),
                            reason: "array schema requires items".into(),
                        })?;
                    format!("[{}]", render_schema(items, tool)?)
                }
                "object" => {
                    let properties = object
                        .get("properties")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let required = required_names(schema, tool)?;
                    let fields = properties
                        .iter()
                        .map(|(name, property)| {
                            let optional = (!required.contains(name)).then_some("?").unwrap_or("");
                            render_field(name, optional, property, tool)
                        })
                        .collect::<Result<Vec<_>>>()?
                        .join(",");
                    let closed = object
                        .get("additionalProperties")
                        .and_then(Value::as_bool)
                        .is_some_and(|value| !value);
                    format!("{{{fields}}}{}", if closed { "!" } else { "" })
                }
                "any" => "any".into(),
                _ => unreachable!("validated type"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join("|"))
}

fn render_field(name: &str, optional: &str, schema: &Value, tool: &str) -> Result<String> {
    let description = schema
        .get("description")
        .and_then(Value::as_str)
        .filter(|description| !description.trim().is_empty())
        .map(|description| format!(" ({description})"))
        .unwrap_or_default();
    Ok(format!(
        "{name}{optional}:{}{}",
        render_schema(schema, tool)?,
        description
    ))
}

fn required_names(schema: &Value, tool: &str) -> Result<BTreeSet<String>> {
    match schema.get("required") {
        Some(required) => Ok(required
            .as_array()
            .ok_or_else(|| Error::UnsupportedSchema {
                tool: tool.into(),
                reason: "invalid required list".into(),
            })?
            .iter()
            .map(|item| {
                item.as_str()
                    .expect("schema was validated before rendering")
                    .to_string()
            })
            .collect()),
        None => Ok(BTreeSet::new()),
    }
}

fn render_enum(values: &Value) -> String {
    let Some(values) = values.as_array() else {
        unreachable!("schema enum was validated")
    };
    let safe_strings: Option<Vec<&str>> = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .filter(|values| {
            values
                .iter()
                .all(|value| !value.is_empty() && value.chars().all(is_enum_token_char))
        });
    match safe_strings {
        Some(values) => values.join("|"),
        None => format!(
            "enum{}",
            serde_json::to_string(values).expect("Value serialization cannot fail")
        ),
    }
}

fn is_enum_token_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
}

fn validate_arguments(tool: &ToolDef, arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        return Err(invalid(
            &tool.function.name,
            "arguments must be a JSON object",
        ));
    }
    if let Some(schema) = &tool.function.parameters {
        validate_value(schema, arguments, "$", &tool.function.name)?;
    }
    Ok(())
}

fn validate_value(schema: &Value, value: &Value, path: &str, tool: &str) -> Result<()> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value)
    {
        return Err(invalid(
            tool,
            format!("{path} is not one of the allowed enum values"),
        ));
    }

    let kinds: Vec<&str> = match schema.get("type") {
        Some(Value::String(kind)) => vec![kind],
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        None if schema.get("properties").is_some() => vec!["object"],
        None => Vec::new(),
        _ => Vec::new(),
    };
    if !kinds.is_empty() && !kinds.iter().any(|kind| value_has_type(value, kind)) {
        return Err(invalid(tool, format!("{path} has the wrong JSON type")));
    }

    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let object = value
            .as_object()
            .ok_or_else(|| invalid(tool, format!("{path} must be an object")))?;
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                return Err(invalid(tool, format!("{path}.{required} is required")));
            }
        }
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false)
            && let Some(unexpected) = object.keys().find(|key| !properties.contains_key(*key))
        {
            return Err(invalid(tool, format!("{path}.{unexpected} is not allowed")));
        }
        for (name, child_schema) in properties {
            if let Some(child) = object.get(name) {
                validate_value(child_schema, child, &format!("{path}.{name}"), tool)?;
            }
        }
    }
    if let Some(items) = schema.get("items") {
        let values = value
            .as_array()
            .ok_or_else(|| invalid(tool, format!("{path} must be an array")))?;
        for (index, item) in values.iter().enumerate() {
            validate_value(items, item, &format!("{path}[{index}]"), tool)?;
        }
    }
    Ok(())
}

fn value_has_type(value: &Value, kind: &str) -> bool {
    match kind {
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

fn invalid(tool: &str, reason: impl Into<String>) -> Error {
    Error::InvalidArguments {
        tool: tool.into(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn calendar() -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create a calendar event.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time"},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]},
                        "metadata": {"type": "object", "properties": {"room": {"type": "string"}}, "required": ["room"]}
                    },
                    "required": ["title", "start"]
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn round_trips_supported_schemas_and_calls() {
        let tools = vec![calendar()];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.prompt.contains("title:str"));
        assert!(compact.prompt.contains("title:str (Event title)"));
        assert!(compact.prompt.contains("visibility?:public|private"));
        assert_eq!(decode_tools(&compact).unwrap(), tools);

        let text = "Before <<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"attendees\":[\"riya@example.com\"],\"metadata\":{\"room\":\"A\"}}>> after";
        assert_eq!(
            decode_calls(text, &tools).unwrap(),
            vec![ToolCall {
                name: "create_calendar_event".into(),
                arguments: json!({"title":"Retro","start":"2026-10-04T10:00:00+05:30","attendees":["riya@example.com"],"metadata":{"room":"A"}})
            }]
        );
    }

    #[test]
    fn rejects_duplicate_tool_names() {
        let tool = calendar();
        assert!(matches!(
            encode_tools(&[tool.clone(), tool]),
            Err(Error::UnsupportedSchema { reason, .. }) if reason == "duplicate tool name"
        ));
    }

    #[test]
    fn string_arguments_can_contain_the_closing_marker() {
        let tools = vec![calendar()];
        let text = "<<call create_calendar_event {\"title\":\"a >> b\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>";
        assert_eq!(
            decode_calls(text, &tools).unwrap()[0].arguments["title"],
            "a >> b"
        );
    }

    #[test]
    fn fails_closed_for_unknown_or_invalid_calls() {
        let tools = vec![calendar()];
        assert!(matches!(
            decode_calls("<<call missing {\"title\":\"x\"}>>", &tools),
            Err(Error::UnknownTool(_))
        ));
        assert!(matches!(
            decode_calls(
                "<<call create_calendar_event {\"start\":\"2026-10-04T10:00:00+05:30\"}>>",
                &tools
            ),
            Err(Error::InvalidArguments { .. })
        ));
        assert!(matches!(
            decode_calls(
                "<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-04T10:00:00+05:30\",\"visibility\":\"secret\"}>>",
                &tools
            ),
            Err(Error::InvalidArguments { .. })
        ));
        assert!(matches!(
            decode_calls(
                "<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-04T10:00:00+05:30\",\"extra\":true}>>",
                &tools
            ),
            Err(Error::InvalidArguments { .. })
        ));
        assert!(matches!(
            decode_calls("<<call create_calendar_event {\"title\":\"x\"}", &tools),
            Err(Error::MalformedCall(_))
        ));
    }

    #[test]
    fn unsupported_schema_features_bypass_compaction() {
        let mut tool = calendar();
        tool.function.parameters.as_mut().unwrap()["properties"]["title"]["pattern"] =
            json!("^[A-Z]");
        assert!(matches!(
            encode_tools(&[tool]),
            Err(Error::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn streams_split_marker_and_multiple_calls() {
        let tools = vec![calendar()];
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        assert!(decoder.push("text <<ca").unwrap().is_empty());
        assert!(
            decoder
                .push("ll create_calendar_event {\"title\":\"Ret")
                .unwrap()
                .is_empty()
        );
        let calls = decoder
            .push("ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> then <<call create_calendar_event {\"title\":\"Two\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>")
            .unwrap();
        assert_eq!(calls.len(), 2);
        assert!(decoder.finish().unwrap().is_empty());
    }

    proptest! {
        #[test]
        fn every_chunk_boundary_decodes_the_same_call(split in 0usize..120) {
            let tools = vec![calendar()];
            let text = "<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>";
            let split = split.min(text.len());
            let mut decoder = StreamDecoder::new(&tools).unwrap();
            let mut calls = decoder.push(&text[..split]).unwrap();
            calls.extend(decoder.push(&text[split..]).unwrap());
            calls.extend(decoder.finish().unwrap());
            prop_assert_eq!(calls.len(), 1);
            prop_assert_eq!(&calls[0].name, "create_calendar_event");
        }
    }
}
