//! Compact representations of function-tool definitions and calls.
//!
//! This crate is independent of the LLM router. It owns the input and output types
//! used by the encoder and decoder; callers convert to provider-specific types at
//! their integration boundary.
//!
//! # Compact grammar
//!
//! Each tool is rendered on one line as `name(parameters) -- "description"`.
//! Parameters use `name:type!` for required fields and `name:type?` for optional
//! fields. Types use `str`, `int`, `num`, and `bool` for common JSON types;
//! `string<format>` preserves a JSON Schema string format, `enum["a","b"]`
//! lists string enum values, `[type]` denotes an array, and `{...}` denotes a
//! nested object whose fields use the same parameter syntax. Omitted additional
//! properties are unrestricted by default; `...!` closes an object, while
//! `...:type` constrains extra properties. Names that are not simple identifiers
//! are JSON-quoted. Descriptions are shortened to remove words already expressed
//! by a field name or type, then emitted as JSON strings. A description on an array
//! item follows its type inside the brackets.
//!
//! # Supported JSON Schema subset
//!
//! Parameters must be an object schema. Properties may use `string` (optionally
//! with `format: date-time` or a string-only `enum`), `integer`, `number`, `boolean`, `null`,
//! arrays with one `items` schema, and nested objects. Objects may use `required`
//! and boolean or schema-valued `additionalProperties`. Tool descriptions and
//! property descriptions are supported; root parameter-schema descriptions are
//! rejected because the current grammar has no unambiguous place for them.
//!
//! Constraints not represented above are rejected rather than discarded. This
//! includes numeric bounds, string length or pattern constraints, array length or
//! uniqueness constraints, tuple arrays, unions (`anyOf`, `oneOf`, `allOf`),
//! references (`$ref`), and tool-level extra fields. String `format` and `enum`
//! cannot be combined. On an encoding error, callers should bypass compaction and
//! send the original tool definition.
//!
//! For example:
//!
//! ```text
//! create_calendar_event(title:str! "Event title", start:string<date-time>! "Start time, ISO 8601", duration_min:int?, attendees:[str]?, visibility:enum["public","private"]?) -- "Create an event in the user's calendar."
//! ```
//!
//! A model emits calls as `<<call EXACT_TOOL_NAME {JSON object}>>`, where
//! `EXACT_TOOL_NAME` is replaced by a listed tool name immediately after `<<call`
//! (not written as `name=...`). Names outside the simple identifier syntax are
//! JSON-quoted. Arguments remain standard
//! JSON; the decoder must recognize the closing marker only after the complete JSON
//! object, so marker-like text inside a quoted JSON string is not mistaken for the
//! end of a call. Calls may appear alongside ordinary text or other calls. Text with
//! no call marker represents no tool calls.

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fmt;
use thiserror::Error;

const CALL_MARKER: &str = "<<call";
const MAX_SCHEMA_DEPTH: usize = 32;

/// An error encountered while converting a schema to the compact grammar.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("cannot compact tool `{tool_name}` at `{path}`: {message}")]
pub struct EncodeError {
    pub tool_name: String,
    pub path: String,
    pub message: String,
}

/// Result type returned by compact-tool encoding operations.
pub type Result<T> = std::result::Result<T, EncodeError>;

/// An error encountered while parsing or validating a compact tool call.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid compact tool call at byte {offset}: {message}")]
pub struct DecodeError {
    pub offset: usize,
    pub message: String,
    incomplete: bool,
}

/// Result type returned by compact-tool decoding operations.
pub type DecodeResult<T> = std::result::Result<T, DecodeError>;

fn function_kind() -> String {
    "function".to_string()
}

/// A function tool definition in the OpenAI-compatible shape accepted by the router.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The function name, description, and JSON Schema for its arguments.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A decoded semantic tool call. The router assigns the provider-facing call ID.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolCall {
    pub name: String,
    /// Parsed JSON arguments; callers serialize this to the provider's required shape.
    pub arguments: Value,
}

/// Compact tool definitions and the instructions for emitting calls in the compact grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub definitions: String,
    pub call_instructions: String,
}

/// Convert function tools to the compact definition grammar.
///
/// Schemas containing unsupported keywords return an error. Callers should use
/// the original definitions unchanged when encoding fails.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut definitions = Vec::with_capacity(tools.len());
    let mut names = BTreeSet::new();
    for tool in tools {
        let name = &tool.function.name;
        if !names.insert(name) {
            return Err(encode_error(name, "name", "tool names must be unique"));
        }
        if tool.kind != "function" {
            return Err(encode_error(
                name,
                "type",
                "only function tools are supported",
            ));
        }
        if !tool.extra.is_empty() {
            return Err(encode_error(
                name,
                "extra",
                "unknown tool-level fields cannot be preserved",
            ));
        }
        if !tool.function.extra.is_empty() {
            return Err(encode_error(
                name,
                "function.extra",
                "unknown function-level fields cannot be preserved",
            ));
        }
        let parameters = tool.function.parameters.as_ref().ok_or_else(|| {
            encode_error(
                name,
                "parameters",
                "a JSON Schema object is required for safe compaction",
            )
        })?;
        let fields = encode_object(parameters, name, "parameters", 0)?;
        let mut definition = format!("{}({fields})", encode_name(name));
        if let Some(description) = &tool.function.description {
            definition.push_str(" -- ");
            definition.push_str(&quote_json(description));
        }
        definitions.push(definition);
    }

    Ok(CompactTools {
        definitions: definitions.join("\n"),
        call_instructions: call_instructions(tools),
    })
}

fn call_instructions(tools: &[ToolDef]) -> String {
    let example_name = tools
        .first()
        .map(|tool| encode_name(&tool.function.name))
        .unwrap_or_else(|| "EXACT_TOOL_NAME".to_string());
    format!(
        "Call `<<call {example_name} {{JSON}}>>` using the exact tool name and matching JSON. No `name=`. One marker per call; otherwise answer normally."
    )
}

/// Decode compact calls and validate each call against its original tool schema.
///
/// Text outside call markers is ignored. An unknown tool, malformed call, or
/// invalid argument returns an error rather than a partial or guessed result.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> DecodeResult<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find(CALL_MARKER) {
        let marker_start = cursor + relative_start;
        let name_start = marker_start + CALL_MARKER.len();
        if !text[name_start..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            return Err(decode_error(
                marker_start,
                "expected whitespace after `<<call`",
            ));
        }

        let name_start = skip_whitespace(text, name_start);
        let (name, name_end) = parse_call_name(text, name_start)?;
        if !text[name_end..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            return Err(decode_error(
                name_end,
                "expected JSON arguments after the tool name",
            ));
        }
        let arguments_start = skip_whitespace(text, name_end);
        let (arguments, marker_end) = parse_call_arguments(text, arguments_start)?;

        let mut matches = tools.iter().filter(|tool| tool.function.name == name);
        let tool = matches
            .next()
            .ok_or_else(|| decode_error(marker_start, &format!("unknown tool `{name}`")))?;
        if matches.next().is_some() {
            return Err(decode_error(
                marker_start,
                &format!("tool name `{name}` is ambiguous"),
            ));
        }
        encode_tools(std::slice::from_ref(tool)).map_err(|error| {
            decode_error(
                marker_start,
                &format!("tool schema cannot be compacted: {error}"),
            )
        })?;
        validate_arguments(tool, &arguments, marker_start)?;

        calls.push(ToolCall { name, arguments });
        cursor = marker_end;
    }
    Ok(calls)
}

/// Incrementally decode complete calls from model-output chunks.
///
/// `push` returns calls as soon as their closing markers arrive. Call `finish`
/// when the stream ends to detect a truncated marker or call.
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    pending: String,
    finished: bool,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef]) -> Self {
        Self {
            tools,
            pending: String::new(),
            finished: false,
        }
    }

    pub fn push(&mut self, chunk: &str) -> DecodeResult<Vec<ToolCall>> {
        if self.finished {
            return Err(decode_error(0, "stream decoder is already finished"));
        }
        self.pending.push_str(chunk);
        let mut decoded = Vec::new();

        loop {
            let Some(start) = self.pending.find(CALL_MARKER) else {
                retain_marker_prefix(&mut self.pending);
                break;
            };
            let mut search_from = start + CALL_MARKER.len();
            let mut consumed = false;
            while let Some(relative_end) = self.pending[search_from..].find(">>") {
                let candidate_end = search_from + relative_end + 2;
                match decode_calls(&self.pending[..candidate_end], self.tools) {
                    Ok(mut calls) if !calls.is_empty() => {
                        decoded.append(&mut calls);
                        self.pending.drain(..candidate_end);
                        consumed = true;
                        break;
                    }
                    Ok(_) => search_from = candidate_end,
                    Err(error) if error.incomplete => search_from = candidate_end,
                    Err(error) => return Err(error),
                }
            }
            if !consumed {
                if start > 0 {
                    self.pending.drain(..start);
                }
                break;
            }
        }
        Ok(decoded)
    }

    pub fn finish(&mut self) -> DecodeResult<Vec<ToolCall>> {
        if self.finished {
            return Err(decode_error(0, "stream decoder is already finished"));
        }
        self.finished = true;
        if self.pending.find(CALL_MARKER).is_some() {
            return decode_calls(&self.pending, self.tools);
        }
        if has_partial_marker_suffix(&self.pending) {
            return Err(incomplete_error(
                self.pending.len(),
                "stream ended in the middle of a call marker",
            ));
        }
        Ok(Vec::new())
    }
}

fn parse_call_name(text: &str, start: usize) -> DecodeResult<(String, usize)> {
    let first = text[start..]
        .chars()
        .next()
        .ok_or_else(|| decode_error(start, "missing tool name"))?;
    if first == '"' {
        let end = scan_json_string(text, start)?;
        let name = serde_json::from_str::<String>(&text[start..end])
            .map_err(|error| decode_error(start, &format!("invalid quoted tool name: {error}")))?;
        return Ok((name, end));
    }

    let end = text[start..]
        .char_indices()
        .find_map(|(offset, character)| character.is_whitespace().then_some(start + offset))
        .unwrap_or(text.len());
    let name = &text[start..end];
    if !is_identifier(name) {
        return Err(decode_error(
            start,
            "tool name must be an identifier or JSON string",
        ));
    }
    Ok((name.to_string(), end))
}

fn scan_json_string(text: &str, start: usize) -> DecodeResult<usize> {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return Ok(index + 1);
        }
        index += 1;
    }
    Err(decode_error(start, "unterminated quoted string"))
}

fn parse_call_arguments(text: &str, start: usize) -> DecodeResult<(Value, usize)> {
    let mut stream =
        serde_json::Deserializer::from_str(&text[start..]).into_iter::<StrictJsonValue>();
    let arguments = match stream.next() {
        Some(Ok(arguments)) => arguments.0,
        Some(Err(error)) if error.is_eof() => {
            return Err(incomplete_error(
                start + stream.byte_offset(),
                error.to_string(),
            ));
        }
        Some(Err(error)) => {
            return Err(decode_error(
                start + stream.byte_offset(),
                &error.to_string(),
            ));
        }
        None => return Err(incomplete_error(start, "missing JSON arguments")),
    };
    let json_end = stream.byte_offset();
    if !arguments.is_object() {
        return Err(decode_error(start, "tool arguments must be a JSON object"));
    }

    let remainder = &text[start + json_end..];
    let whitespace_len = remainder.len() - remainder.trim_start().len();
    let close_start = start + json_end + whitespace_len;
    if !text[close_start..].starts_with(">>") {
        if text[close_start..].is_empty() {
            return Err(incomplete_error(
                close_start,
                "expected `>>` after the JSON arguments",
            ));
        }
        return Err(decode_error(
            close_start,
            "expected `>>` after the JSON arguments",
        ));
    }
    Ok((arguments, close_start + 2))
}

fn skip_whitespace(text: &str, start: usize) -> usize {
    text[start..]
        .char_indices()
        .find_map(|(offset, character)| (!character.is_whitespace()).then_some(start + offset))
        .unwrap_or(text.len())
}

fn retain_marker_prefix(pending: &mut String) {
    let keep = (1..CALL_MARKER.len())
        .rev()
        .find(|length| pending.ends_with(&CALL_MARKER[..*length]))
        .unwrap_or(0);
    if keep == 0 {
        pending.clear();
    } else {
        pending.drain(..pending.len() - keep);
    }
}

fn has_partial_marker_suffix(pending: &str) -> bool {
    (1..CALL_MARKER.len()).any(|length| pending.ends_with(&CALL_MARKER[..length]))
}

fn validate_arguments(tool: &ToolDef, arguments: &Value, offset: usize) -> DecodeResult<()> {
    let schema = tool
        .function
        .parameters
        .as_ref()
        .ok_or_else(|| decode_error(offset, "tool is missing its parameter schema"))?;
    validate_object(
        schema,
        arguments,
        &tool.function.name,
        "arguments",
        offset,
        0,
    )
}

fn validate_schema_value(
    schema: &Value,
    value: &Value,
    tool_name: &str,
    path: &str,
    offset: usize,
    depth: usize,
) -> DecodeResult<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(decode_error(
            offset,
            "schema nesting exceeds the supported depth",
        ));
    }
    let schema_object = schema.as_object().ok_or_else(|| {
        decode_error(
            offset,
            &format!("{tool_name} schema at `{path}` is not an object"),
        )
    })?;
    let schema_type = schema_object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            decode_error(
                offset,
                &format!("{tool_name} schema at `{path}` has no type"),
            )
        })?;

    let valid = match schema_type {
        "string" => {
            let Some(string) = value.as_str() else {
                return invalid_value(offset, tool_name, path, "expected a string");
            };
            if schema_object.get("format").and_then(Value::as_str) == Some("date-time")
                && chrono::DateTime::parse_from_rfc3339(string).is_err()
            {
                return invalid_value(offset, tool_name, path, "expected an RFC3339 date-time");
            }
            if let Some(values) = schema_object.get("enum") {
                values.as_array().is_some_and(|values| {
                    values
                        .iter()
                        .any(|candidate| candidate.as_str() == Some(string))
                })
            } else {
                true
            }
        }
        "integer" => {
            value.as_i64().is_some()
                || value.as_u64().is_some()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => {
            let Some(values) = value.as_array() else {
                return invalid_value(offset, tool_name, path, "expected an array");
            };
            let items = schema_object.get("items").ok_or_else(|| {
                decode_error(
                    offset,
                    &format!("{tool_name} schema at `{path}` has no items schema"),
                )
            })?;
            for (index, item) in values.iter().enumerate() {
                validate_schema_value(
                    items,
                    item,
                    tool_name,
                    &format!("{path}[{index}]"),
                    offset,
                    depth + 1,
                )?;
            }
            true
        }
        "object" => {
            validate_object(schema, value, tool_name, path, offset, depth + 1)?;
            true
        }
        _ => return invalid_value(offset, tool_name, path, "schema type is unsupported"),
    };

    if valid {
        Ok(())
    } else {
        invalid_value(offset, tool_name, path, "value does not match the schema")
    }
}

fn validate_object(
    schema: &Value,
    value: &Value,
    tool_name: &str,
    path: &str,
    offset: usize,
    depth: usize,
) -> DecodeResult<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(decode_error(
            offset,
            "schema nesting exceeds the supported depth",
        ));
    }
    let value = value
        .as_object()
        .ok_or_else(|| decode_error(offset, &format!("{tool_name} `{path}` must be an object")))?;
    let schema = schema.as_object().ok_or_else(|| {
        decode_error(
            offset,
            &format!("{tool_name} schema at `{path}` is not an object"),
        )
    })?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !value.contains_key(name) {
                return invalid_value(
                    offset,
                    tool_name,
                    &format!("{path}.{name}"),
                    "required argument is missing",
                );
            }
        }
    }

    for (name, argument) in value {
        let argument_path = format!("{path}.{name}");
        if let Some(property_schema) = properties.get(name) {
            validate_schema_value(
                property_schema,
                argument,
                tool_name,
                &argument_path,
                offset,
                depth + 1,
            )?;
        } else {
            match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    return invalid_value(
                        offset,
                        tool_name,
                        &argument_path,
                        "additional argument is not allowed",
                    );
                }
                Some(Value::Bool(true)) | None => {}
                Some(additional_schema) => validate_schema_value(
                    additional_schema,
                    argument,
                    tool_name,
                    &argument_path,
                    offset,
                    depth + 1,
                )?,
            }
        }
    }
    Ok(())
}

fn invalid_value<T>(offset: usize, tool_name: &str, path: &str, reason: &str) -> DecodeResult<T> {
    Err(decode_error(
        offset,
        &format!("{tool_name} argument `{path}`: {reason}"),
    ))
}

fn decode_error(offset: usize, message: &str) -> DecodeError {
    DecodeError {
        offset,
        message: message.to_string(),
        incomplete: false,
    }
}

fn incomplete_error(offset: usize, message: impl Into<String>) -> DecodeError {
    DecodeError {
        offset,
        message: message.into(),
        incomplete: true,
    }
}

struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        let number = serde_json::Number::from_f64(value)
            .ok_or_else(|| E::custom("non-finite numbers are not valid JSON"))?;
        Ok(StrictJsonValue(Value::Number(number)))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map_access: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = map_access.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate object key `{key}`")));
            }
            let value = map_access.next_value::<StrictJsonValue>()?;
            values.insert(key, value.0);
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

fn encode_object(schema: &Value, tool_name: &str, path: &str, depth: usize) -> Result<String> {
    ensure_depth(tool_name, path, depth)?;
    let object = schema
        .as_object()
        .ok_or_else(|| encode_error(tool_name, path, "expected a JSON Schema object"))?;
    ensure_supported_keys(
        object,
        &["type", "properties", "required", "additionalProperties"],
        tool_name,
        path,
    )?;
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(encode_error(
            tool_name,
            path,
            "tool parameters must have type object",
        ));
    }

    let properties = match object.get("properties") {
        Some(value) => value
            .as_object()
            .ok_or_else(|| encode_error(tool_name, path, "properties must be an object"))?,
        None => {
            let empty = Map::new();
            return encode_object_fields(object, &empty, tool_name, path, depth);
        }
    };
    encode_object_fields(object, properties, tool_name, path, depth)
}

fn encode_object_fields(
    object: &Map<String, Value>,
    properties: &Map<String, Value>,
    tool_name: &str,
    path: &str,
    depth: usize,
) -> Result<String> {
    let required = match object.get("required") {
        Some(value) => {
            let values = value
                .as_array()
                .ok_or_else(|| encode_error(tool_name, path, "required must be an array"))?;
            let mut names = BTreeSet::new();
            for value in values {
                let name = value.as_str().ok_or_else(|| {
                    encode_error(tool_name, path, "required entries must be strings")
                })?;
                if !names.insert(name.to_string()) {
                    return Err(encode_error(
                        tool_name,
                        path,
                        "required contains duplicates",
                    ));
                }
                if !properties.contains_key(name) {
                    return Err(encode_error(
                        tool_name,
                        path,
                        "required names must exist in properties",
                    ));
                }
            }
            names
        }
        None => BTreeSet::new(),
    };

    let mut fields = Vec::with_capacity(properties.len() + 1);
    for (name, property_schema) in properties {
        let property_path = format!("{path}.properties.{name}");
        let (kind, description) =
            encode_schema(property_schema, tool_name, &property_path, depth + 1)?;
        let marker = if required.contains(name) { '!' } else { '?' };
        let mut field = format!("{}:{kind}{marker}", encode_name(name));
        if let Some(description) = description
            .as_deref()
            .and_then(|description| compact_description(name, &kind, description))
        {
            field.push(' ');
            field.push_str(&quote_json(&description));
        }
        fields.push(field);
    }

    match object.get("additionalProperties") {
        None | Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => fields.push("...!".to_string()),
        Some(schema) => {
            let property_path = format!("{path}.additionalProperties");
            let (kind, description) = encode_schema(schema, tool_name, &property_path, depth + 1)?;
            let mut field = format!("...:{kind}");
            if let Some(description) = description {
                field.push(' ');
                field.push_str(&quote_json(&description));
            }
            fields.push(field);
        }
    }
    Ok(fields.join(", "))
}

fn compact_description(name: &str, kind: &str, description: &str) -> Option<String> {
    let mut represented = description_terms(name);
    represented.extend(description_terms(kind));
    if name.split('_').any(|part| part == "min") {
        represented.insert("minutes".to_string());
    }
    if name.ends_with('s') {
        represented.insert(name.trim_end_matches('s').to_ascii_lowercase());
    }

    let terms: Vec<&str> = description
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .filter(|term| {
            let normalized = term.to_ascii_lowercase();
            !represented.contains(&normalized)
                && !matches!(
                    normalized.as_str(),
                    "a" | "an" | "the" | "in" | "of" | "for" | "from" | "to" | "and"
                )
        })
        .collect();
    if terms.is_empty()
        || terms
            .iter()
            .all(|term| matches!(*term, "line" | "field" | "value"))
    {
        None
    } else {
        Some(terms.join(" "))
    }
}

fn description_terms(value: &str) -> BTreeSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn encode_schema(
    schema: &Value,
    tool_name: &str,
    path: &str,
    depth: usize,
) -> Result<(String, Option<String>)> {
    ensure_depth(tool_name, path, depth)?;
    let object = schema
        .as_object()
        .ok_or_else(|| encode_error(tool_name, path, "expected a JSON Schema object"))?;
    let description = match object.get("description") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| encode_error(tool_name, path, "description must be a string"))?
                .to_string(),
        ),
        None => None,
    };
    let schema_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| encode_error(tool_name, path, "a string type is required"))?;

    let kind = match schema_type {
        "string" => {
            ensure_supported_keys(
                object,
                &["type", "description", "format", "enum"],
                tool_name,
                path,
            )?;
            match (object.get("format"), object.get("enum")) {
                (Some(_), Some(_)) => {
                    return Err(encode_error(
                        tool_name,
                        path,
                        "string format and enum cannot be combined",
                    ));
                }
                (Some(format), None) => {
                    let format = format
                        .as_str()
                        .ok_or_else(|| encode_error(tool_name, path, "format must be a string"))?;
                    if format != "date-time" {
                        return Err(encode_error(
                            tool_name,
                            path,
                            "only the date-time string format is supported",
                        ));
                    }
                    if !is_identifier(format) {
                        return Err(encode_error(
                            tool_name,
                            path,
                            "format contains characters unsupported by the compact grammar",
                        ));
                    }
                    format!("string<{format}>")
                }
                (None, Some(values)) => encode_string_enum(values, tool_name, path)?,
                (None, None) => "str".to_string(),
            }
        }
        "integer" => {
            ensure_supported_keys(object, &["type", "description"], tool_name, path)?;
            "int".to_string()
        }
        "number" => {
            ensure_supported_keys(object, &["type", "description"], tool_name, path)?;
            "num".to_string()
        }
        "boolean" => {
            ensure_supported_keys(object, &["type", "description"], tool_name, path)?;
            "bool".to_string()
        }
        "null" => {
            ensure_supported_keys(object, &["type", "description"], tool_name, path)?;
            "null".to_string()
        }
        "array" => {
            ensure_supported_keys(object, &["type", "description", "items"], tool_name, path)?;
            let items = object
                .get("items")
                .ok_or_else(|| encode_error(tool_name, path, "array items schema is required"))?;
            let (item_type, item_description) =
                encode_schema(items, tool_name, &format!("{path}.items"), depth + 1)?;
            let item = match item_description {
                Some(description) => format!("{item_type} {}", quote_json(&description)),
                None => item_type,
            };
            format!("[{item}]")
        }
        "object" => {
            ensure_supported_keys(
                object,
                &[
                    "type",
                    "description",
                    "properties",
                    "required",
                    "additionalProperties",
                ],
                tool_name,
                path,
            )?;
            format!(
                "{{{}}}",
                encode_object_fields_for_nested(object, tool_name, path, depth)?
            )
        }
        _ => {
            return Err(encode_error(
                tool_name,
                path,
                "schema type is not supported by the compact grammar",
            ));
        }
    };
    Ok((kind, description))
}

fn encode_object_fields_for_nested(
    object: &Map<String, Value>,
    tool_name: &str,
    path: &str,
    depth: usize,
) -> Result<String> {
    let properties = match object.get("properties") {
        Some(value) => value
            .as_object()
            .ok_or_else(|| encode_error(tool_name, path, "properties must be an object"))?,
        None => {
            let empty = Map::new();
            return encode_object_fields(object, &empty, tool_name, path, depth);
        }
    };
    encode_object_fields(object, properties, tool_name, path, depth)
}

fn encode_string_enum(values: &Value, tool_name: &str, path: &str) -> Result<String> {
    let values = values
        .as_array()
        .ok_or_else(|| encode_error(tool_name, path, "enum must be an array"))?;
    if values.is_empty() {
        return Err(encode_error(tool_name, path, "enum must not be empty"));
    }
    let mut members = Vec::with_capacity(values.len());
    for value in values {
        if !value.is_string() {
            return Err(encode_error(
                tool_name,
                path,
                "only string enum values are supported",
            ));
        }
        members.push(quote_json(value.as_str().unwrap_or_default()));
    }
    Ok(format!("enum[{}]", members.join(",")))
}

fn ensure_supported_keys(
    object: &Map<String, Value>,
    supported: &[&str],
    tool_name: &str,
    path: &str,
) -> Result<()> {
    if let Some(key) = object.keys().find(|key| !supported.contains(&key.as_str())) {
        return Err(encode_error(
            tool_name,
            &format!("{path}.{key}"),
            "schema keyword is not supported by the compact grammar",
        ));
    }
    Ok(())
}

fn ensure_depth(tool_name: &str, path: &str, depth: usize) -> Result<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(encode_error(
            tool_name,
            path,
            "schema nesting exceeds the supported depth",
        ));
    }
    Ok(())
}

fn encode_name(name: &str) -> String {
    if is_identifier(name) {
        name.to_string()
    } else {
        quote_json(name)
    }
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn quote_json(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn encode_error(tool_name: &str, path: &str, message: &str) -> EncodeError {
    EncodeError {
        tool_name: tool_name.to_string(),
        path: path.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn tool(parameters: Value) -> ToolDef {
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "create_event".to_string(),
                description: Some("Create an event.".to_string()),
                parameters: Some(parameters),
                extra: Map::new(),
            },
            extra: Map::new(),
        }
    }

    #[test]
    fn encodes_supported_schema_and_preserves_descriptions() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "location": {
                    "type": "object",
                    "properties": {"room": {"type": "string"}},
                    "required": ["room"],
                    "additionalProperties": false
                }
            },
            "required": ["title", "start"],
            "additionalProperties": false
        }))];

        let compact = encode_tools(&tools).expect("valid supported schema");
        assert_eq!(
            compact.definitions,
            "create_event(attendees:[str]?, duration_min:int?, location:{room:str!, ...!}?, start:string<date-time>!, title:str! \"Event\", visibility:enum[\"public\",\"private\"]?, ...!) -- \"Create an event.\""
        );
        assert!(compact.call_instructions.contains("<<call create_event"));
        assert!(compact.call_instructions.contains("No `name=`"));
    }

    #[test]
    fn preserves_open_additional_properties() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {},
            "required": []
        }))];
        let compact = encode_tools(&tools).expect("valid open object schema");
        assert_eq!(
            compact.definitions,
            "create_event() -- \"Create an event.\""
        );
    }

    #[test]
    fn marks_closed_objects_and_shortens_redundant_descriptions() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "title": {"type": "string", "description": "Event title"}
            },
            "required": ["title"],
            "additionalProperties": false
        }))];
        let compact = encode_tools(&tools).expect("valid closed object schema");
        assert_eq!(
            compact.definitions,
            "create_event(duration_min:int?, title:str! \"Event\", ...!) -- \"Create an event.\""
        );
    }

    #[test]
    fn rejects_unrepresented_constraints() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {"count": {"type": "integer", "minimum": 1}},
            "required": ["count"]
        }))];
        let error = encode_tools(&tools).expect_err("minimum must not be silently dropped");
        assert_eq!(error.path, "parameters.properties.count.minimum");
    }

    #[test]
    fn rejects_root_schema_description_instead_of_dropping_it() {
        let tools = [tool(json!({
            "type": "object",
            "description": "Arguments for an event",
            "properties": {},
            "additionalProperties": false
        }))];
        let error = encode_tools(&tools).expect_err("root descriptions are not yet rendered");
        assert_eq!(error.path, "parameters.description");
    }

    #[test]
    fn rejects_unknown_function_fields_instead_of_dropping_them() {
        let mut tool_json = json!({
            "type": "function",
            "function": {
                "name": "create_event",
                "description": "Create an event.",
                "parameters": {"type": "object", "properties": {}},
                "strict": true
            }
        });
        let tool: ToolDef = serde_json::from_value(tool_json.take()).expect("valid tool JSON");
        let error = encode_tools(&[tool]).expect_err("unknown function fields must bypass");
        assert_eq!(error.path, "function.extra");
    }

    #[test]
    fn validates_date_time_format_in_arguments() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {"start": {"type": "string", "format": "date-time"}},
            "required": ["start"],
            "additionalProperties": false
        }))];

        assert!(
            decode_calls(
                "<<call create_event {\"start\":\"2026-10-05T15:00:00+05:30\"}>>",
                &tools
            )
            .is_ok()
        );
        let error = decode_calls(
            "<<call create_event {\"start\":\"next Monday at 3pm\"}>>",
            &tools,
        )
        .expect_err("invalid date-time must fail closed");
        assert!(error.message.contains("RFC3339 date-time"));
    }

    #[test]
    fn decodes_multiple_calls_and_ignores_surrounding_text() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "metadata": {
                    "type": "object",
                    "properties": {"room": {"type": "string"}},
                    "required": ["room"],
                    "additionalProperties": false
                }
            },
            "required": ["title"],
            "additionalProperties": false
        }))];
        let decoded = decode_calls(
            "Before <<call create_event {\"title\":\"Design >> review\",\"visibility\":\"private\",\"attendees\":[\"riya@example.com\"],\"metadata\":{\"room\":\"A\"}}>> between <<call create_event {\"title\":\"Retro\"}>> after",
            &tools,
        )
        .expect("valid calls with text around them");

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].name, "create_event");
        assert_eq!(
            decoded[0].arguments,
            json!({
                "title": "Design >> review",
                "visibility": "private",
                "attendees": ["riya@example.com"],
                "metadata": {"room": "A"}
            })
        );
        assert_eq!(decoded[1].arguments, json!({"title": "Retro"}));
    }

    #[test]
    fn plain_text_decodes_to_no_calls() {
        assert!(
            decode_calls("No tool is needed.", &[])
                .expect("plain text is a valid no-call response")
                .is_empty()
        );
    }

    #[test]
    fn supports_json_quoted_tool_names() {
        let mut named_tool = tool(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }));
        named_tool.function.name = "tool with spaces".to_string();
        let tools = [named_tool];

        let compact = encode_tools(&tools).expect("quoted names can be encoded");
        assert!(compact.definitions.starts_with("\"tool with spaces\"("));
        let calls = decode_calls("<<call \"tool with spaces\" {}>>", &tools)
            .expect("quoted names can be decoded");
        assert_eq!(calls[0].name, "tool with spaces");
    }

    #[test]
    fn rejects_unknown_tools_and_invalid_arguments() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "attendees": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["title"],
            "additionalProperties": false
        }))];

        for (text, expected) in [
            ("<<call missing {}>>", "unknown tool"),
            ("<<call create_event {}>>", "required argument is missing"),
            (
                "<<call create_event {\"title\":\"Retro\",\"visibility\":\"secret\"}>>",
                "value does not match the schema",
            ),
            (
                "<<call create_event {\"title\":\"Retro\",\"other\":true}>>",
                "additional argument is not allowed",
            ),
            (
                "<<call create_event {\"title\":\"Retro\",\"attendees\":[1]}>>",
                "expected a string",
            ),
        ] {
            let error = decode_calls(text, &tools).expect_err("invalid calls must fail closed");
            assert!(error.message.contains(expected), "{error}");
        }
    }

    #[test]
    fn rejects_duplicate_json_keys_and_malformed_markers() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
            "additionalProperties": false
        }))];
        let duplicate = decode_calls(
            "<<call create_event {\"title\":\"first\",\"title\":\"second\"}>>",
            &tools,
        )
        .expect_err("duplicate keys must not be interpreted differently by parsers");
        assert!(duplicate.message.contains("duplicate object key"));

        let malformed = decode_calls("<<call create_event {\"title\":\"Retro\"}", &tools)
            .expect_err("the call marker must be closed");
        assert!(malformed.message.contains("expected `>>`"));
    }

    #[test]
    fn stream_decoder_handles_split_markers_and_json_strings() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
            "additionalProperties": false
        }))];
        let mut decoder = StreamDecoder::new(&tools);
        let mut calls = Vec::new();
        for chunk in [
            "answer <",
            "<ca",
            "ll create_event {\"title\":\"inside >> string\"}",
            ">",
            "> trailing text",
        ] {
            calls.extend(decoder.push(chunk).expect("chunk is incomplete or valid"));
        }
        calls.extend(decoder.finish().expect("stream is complete"));

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], "inside >> string");
    }

    #[test]
    fn stream_decoder_rejects_a_truncated_marker() {
        let tools = [tool(json!({"type": "object", "properties": {}}))];
        let mut decoder = StreamDecoder::new(&tools);
        assert!(
            decoder
                .push("answer <<ca")
                .expect("partial marker")
                .is_empty()
        );
        let error = decoder
            .finish()
            .expect_err("truncated marker must fail closed");
        assert!(error.incomplete);
    }

    #[test]
    fn decodes_escaped_quotes_backslashes_and_unicode() {
        let tools = [tool(json!({
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
            "additionalProperties": false
        }))];
        let decoded = decode_calls(
            r#"<<call create_event {"title":"quote: \" slash: \\ unicode: café"}>>"#,
            &tools,
        )
        .expect("escaped JSON strings should decode");

        assert_eq!(
            decoded[0].arguments,
            json!({"title": "quote: \" slash: \\ unicode: café"})
        );
    }

    proptest! {
        #[test]
        fn json_string_arguments_roundtrip(value in ".*") {
            let tools = [tool(json!({
                "type": "object",
                "properties": {"title": {"type": "string"}},
                "required": ["title"],
                "additionalProperties": false
            }))];
            let arguments = json!({"title": value});
            let encoded = serde_json::to_string(&arguments).expect("JSON value serializes");
            let text = format!("<<call create_event {encoded}>>");
            let decoded = decode_calls(&text, &tools).expect("serialized JSON round-trips");

            prop_assert_eq!(&decoded[0].arguments, &arguments);
        }
    }
}
