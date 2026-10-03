//! Compact, fail-closed function tool schemas.
//!
//! This crate intentionally owns small OpenAI-shaped types instead of depending on
//! `nasiko-llm-router`: the router can convert at its boundary while this codec
//! remains usable and testable by itself.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// OpenAI-shaped function tool definition owned by this standalone crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The callable portion of a tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded call. Arguments stay structured until a router adapter serializes
/// them to OpenAI's `function.arguments` JSON-string field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// The compact prompt material plus an exact copy of the original schemas.
/// Keeping the original is what lets `decode_tools` prove that no schema data was
/// silently discarded and lets decoding validate against the real schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactTools {
    pub prompt: String,
    #[serde(skip)]
    tools: Vec<ToolDef>,
}

/// Errors are deliberately categorical so an egress adapter can fail closed
/// without inventing a call or leaking parser internals to a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnsupportedSchema(String),
    UnknownTool(String),
    InvalidArguments(String),
    MalformedOutput(String),
}

impl Error {
    /// Stable machine-readable category used by the evaluation example.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedSchema(_) => "unsupported_schema",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::MalformedOutput(_) => "malformed_output",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(message) => write!(f, "unsupported schema: {message}"),
            Self::UnknownTool(name) => write!(f, "unknown tool: {name}"),
            Self::InvalidArguments(message) => write!(f, "invalid arguments: {message}"),
            Self::MalformedOutput(message) => write!(f, "malformed compact call: {message}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Incremental decoder. It deliberately buffers until `finish`: emitting a call
/// before its final `>>` would make an incomplete streamed argument observable.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    text: String,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            text: String::new(),
        }
    }

    /// Add the next provider text chunk. Chunks may split the marker, name, JSON,
    /// or closing delimiter at any byte boundary that preserves UTF-8.
    pub fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
    }

    /// Decode every complete call after the provider has ended the response.
    pub fn finish(&self) -> Result<Vec<ToolCall>> {
        decode_calls(&self.text, &self.tools)
    }
}

/// Encode supported tool schemas into a concise, model-readable inventory.
/// Unsupported schema features are rejected so the caller can send native tools.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut names = BTreeSet::new();
    let mut lines = Vec::with_capacity(tools.len());

    for tool in tools {
        if tool.kind != "function" {
            return Err(Error::UnsupportedSchema(format!(
                "tool '{}' is not a function",
                tool.function.name
            )));
        }
        if !valid_name(&tool.function.name) {
            return Err(Error::UnsupportedSchema(format!(
                "tool name '{}' is not valid in compact grammar",
                tool.function.name
            )));
        }
        if !names.insert(tool.function.name.as_str()) {
            return Err(Error::UnsupportedSchema(format!(
                "duplicate tool name '{}'",
                tool.function.name
            )));
        }

        let signature = match tool.function.parameters.as_ref() {
            Some(schema) => render_root_schema(schema)?,
            None => String::new(),
        };
        let description = tool
            .function
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(|description| format!(" — {description}"))
            .unwrap_or_default();
        lines.push(format!(
            "{}({}){}",
            tool.function.name, signature, description
        ));
    }

    Ok(CompactTools {
        prompt: format!(
            "Tools (`?` = optional):\n{}\nCall: <<call NAME {{JSON object}}>>. Use plain text when no tool is needed.",
            lines.join("\n")
        ),
        tools: tools.to_vec(),
    })
}

/// Return the original schemas stored during encoding, allowing an exact
/// schema-preservation check without attempting to reconstruct JSON Schema from
/// a display-oriented compact signature.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    // CompactTools is constructed only by `encode_tools`; validating again makes
    // this public API robust if a future deserialization route is introduced.
    for tool in &compact.tools {
        if let Some(schema) = tool.function.parameters.as_ref() {
            render_root_schema(schema)?;
        }
    }
    Ok(compact.tools.clone())
}

/// Decode compact calls in text and validate their JSON arguments against the
/// original schemas. Plain text answers return an empty vector.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut offset = 0;

    while let Some(relative_marker) = text[offset..].find("<<call") {
        let marker = offset + relative_marker;
        let mut cursor = marker + "<<call".len();
        if !starts_with_whitespace(text, cursor) {
            return Err(Error::MalformedOutput(
                "expected whitespace after <<call".into(),
            ));
        }
        cursor = skip_whitespace(text, cursor);

        let name_start = cursor;
        while let Some(ch) = text[cursor..].chars().next() {
            if ch.is_whitespace() {
                break;
            }
            cursor += ch.len_utf8();
        }
        if name_start == cursor {
            return Err(Error::MalformedOutput("missing tool name".into()));
        }
        let name = &text[name_start..cursor];
        if !valid_name(name) {
            return Err(Error::MalformedOutput("invalid tool name".into()));
        }
        let tool = tools
            .iter()
            .find(|tool| tool.function.name == name)
            .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

        if !starts_with_whitespace(text, cursor) {
            return Err(Error::MalformedOutput(
                "expected whitespace before arguments".into(),
            ));
        }
        cursor = skip_whitespace(text, cursor);
        if !text[cursor..].starts_with('{') {
            return Err(Error::MalformedOutput(
                "arguments must be a JSON object".into(),
            ));
        }

        let (arguments, consumed) = parse_json_value(&text[cursor..])?;
        if !arguments.is_object() {
            return Err(Error::InvalidArguments(
                "arguments must be an object".into(),
            ));
        }
        cursor += consumed;
        cursor = skip_whitespace(text, cursor);
        if !text[cursor..].starts_with(">>") {
            return Err(Error::MalformedOutput("missing closing >>".into()));
        }
        cursor += 2;

        validate_arguments(tool, &arguments)?;
        calls.push(ToolCall {
            name: name.to_string(),
            arguments,
        });
        offset = cursor;
    }

    Ok(calls)
}

fn function_kind() -> String {
    "function".into()
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z' | 'A'..='Z' | '_'))
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn skip_whitespace(text: &str, mut cursor: usize) -> usize {
    while let Some(ch) = text[cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    cursor
}

fn starts_with_whitespace(text: &str, cursor: usize) -> bool {
    text[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
}

fn parse_json_value(text: &str) -> Result<(Value, usize)> {
    let mut stream = serde_json::Deserializer::from_str(text).into_iter::<Value>();
    let value = stream
        .next()
        .ok_or_else(|| Error::MalformedOutput("missing JSON arguments".into()))
        .map_err(|_| Error::MalformedOutput("invalid JSON arguments".into()))?;
    let value = value.map_err(|_| Error::MalformedOutput("invalid JSON arguments".into()))?;
    Ok((value, stream.byte_offset()))
}

fn render_root_schema(schema: &Value) -> Result<String> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::UnsupportedSchema("parameters must be a JSON object".into()))?;
    validate_schema_keywords(object, true)?;
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::UnsupportedSchema(
            "parameters must have type object".into(),
        ));
    }
    render_object_fields(object)
}

fn render_schema(schema: &Value) -> Result<String> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::UnsupportedSchema("schema node must be an object".into()))?;
    validate_schema_keywords(object, false)?;

    let base = match object.get("type").and_then(Value::as_str) {
        Some("string") => "str".into(),
        Some("integer") => "int".into(),
        Some("number") => "num".into(),
        Some("boolean") => "bool".into(),
        Some("null") => "null".into(),
        Some("array") => format!("[{}]", render_schema(required_value(object, "items")?)?),
        Some("object") => format!("{{{}}}", render_object_fields(object)?),
        Some(other) => {
            return Err(Error::UnsupportedSchema(format!(
                "unsupported type '{other}'"
            )));
        }
        None if object.contains_key("enum") => "enum".into(),
        None => return Err(Error::UnsupportedSchema("schema node has no type".into())),
    };

    let formatted = match object.get("format").and_then(Value::as_str) {
        Some(format) => format!("{base}<{format}>"),
        None => base,
    };
    match object.get("enum") {
        Some(values) => Ok(format!("{}={}", formatted, render_enum(values)?)),
        None => Ok(formatted),
    }
}

fn render_object_fields(schema: &Map<String, Value>) -> Result<String> {
    let required = required_names(schema)?;
    let properties = match schema.get("properties") {
        Some(Value::Object(properties)) => properties,
        Some(_) => {
            return Err(Error::UnsupportedSchema(
                "properties must be an object".into(),
            ));
        }
        None if required.is_empty() => return Ok(String::new()),
        None => {
            return Err(Error::UnsupportedSchema(
                "required properties must be declared in properties".into(),
            ));
        }
    };
    for name in &required {
        if !properties.contains_key(*name) {
            return Err(Error::UnsupportedSchema(format!(
                "required property '{name}' is not declared"
            )));
        }
    }
    let mut fields = Vec::with_capacity(properties.len());
    for (name, child) in properties {
        if !valid_name(name) {
            return Err(Error::UnsupportedSchema(format!(
                "property name '{name}' is not valid in compact grammar"
            )));
        }
        let suffix = if required.contains(name.as_str()) {
            ""
        } else {
            "?"
        };
        let description = child
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(|description| format!("[{description}]"))
            .unwrap_or_default();
        fields.push(format!(
            "{name}{suffix}:{}{}",
            render_schema(child)?,
            description
        ));
    }
    Ok(fields.join(", "))
}

fn render_enum(values: &Value) -> Result<String> {
    let values = values
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| Error::UnsupportedSchema("enum must be a non-empty array".into()))?;
    values
        .iter()
        .map(|value| match value {
            Value::String(value) => Ok(value.clone()),
            Value::Number(value) => Ok(value.to_string()),
            Value::Bool(value) => Ok(value.to_string()),
            Value::Null => Ok("null".into()),
            _ => Err(Error::UnsupportedSchema(
                "enum values must be scalar".into(),
            )),
        })
        .collect::<Result<Vec<_>>>()
        .map(|values| values.join("|"))
}

fn required_value<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a Value> {
    object
        .get(key)
        .ok_or_else(|| Error::UnsupportedSchema(format!("{key} is required for this schema type")))
}

fn required_names(schema: &Map<String, Value>) -> Result<BTreeSet<&str>> {
    match schema.get("required") {
        None => Ok(BTreeSet::new()),
        Some(Value::Array(names)) => names
            .iter()
            .map(|name| {
                name.as_str().ok_or_else(|| {
                    Error::UnsupportedSchema("required entries must be strings".into())
                })
            })
            .collect(),
        Some(_) => Err(Error::UnsupportedSchema("required must be an array".into())),
    }
}

fn validate_schema_keywords(schema: &Map<String, Value>, root: bool) -> Result<()> {
    const ALLOWED: &[&str] = &[
        "type",
        "description",
        "format",
        "enum",
        "properties",
        "required",
        "items",
        "additionalProperties",
        "title",
        "default",
    ];
    for key in schema.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(Error::UnsupportedSchema(format!(
                "keyword '{key}' is not supported"
            )));
        }
    }
    if let Some(value) = schema.get("additionalProperties")
        && !value.is_boolean()
    {
        return Err(Error::UnsupportedSchema(
            "non-boolean additionalProperties is not supported".into(),
        ));
    }
    if !root && let Some(Value::Array(types)) = schema.get("type") {
        if types.len() > 1 {
            return Err(Error::UnsupportedSchema(
                "union type is not supported".into(),
            ));
        }
    }
    Ok(())
}

fn validate_arguments(tool: &ToolDef, arguments: &Value) -> Result<()> {
    let Some(schema) = tool.function.parameters.as_ref() else {
        return Ok(());
    };
    validate_schema(schema, arguments, "arguments")
}

fn validate_schema(schema: &Value, value: &Value, path: &str) -> Result<()> {
    let object = schema
        .as_object()
        .ok_or_else(|| Error::InvalidArguments(format!("{path} has an invalid schema")))?;
    validate_schema_keywords(object, false).map_err(|error| match error {
        Error::UnsupportedSchema(message) => Error::InvalidArguments(message),
        other => other,
    })?;

    if let Some(enum_values) = object.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or_else(|| Error::InvalidArguments(format!("{path} enum is not an array")))?;
        if !values.iter().any(|candidate| candidate == value) {
            return Err(Error::InvalidArguments(format!(
                "{path} is not an allowed enum value"
            )));
        }
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") if value.is_string() => Ok(()),
        Some("integer") if value.as_i64().is_some() || value.as_u64().is_some() => Ok(()),
        Some("number") if value.is_number() => Ok(()),
        Some("boolean") if value.is_boolean() => Ok(()),
        Some("null") if value.is_null() => Ok(()),
        Some("array") => validate_array(object, value, path),
        Some("object") => validate_object(object, value, path),
        Some(kind) => Err(Error::InvalidArguments(format!("{path} must be {kind}"))),
        None if object.contains_key("enum") => Ok(()),
        None => Err(Error::InvalidArguments(format!(
            "{path} schema has no type"
        ))),
    }
}

fn validate_array(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<()> {
    let values = value
        .as_array()
        .ok_or_else(|| Error::InvalidArguments(format!("{path} must be an array")))?;
    let items = schema
        .get("items")
        .ok_or_else(|| Error::InvalidArguments(format!("{path} items schema is missing")))?;
    for (index, value) in values.iter().enumerate() {
        validate_schema(items, value, &format!("{path}[{index}]"))?;
    }
    Ok(())
}

fn validate_object(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<()> {
    let fields = value
        .as_object()
        .ok_or_else(|| Error::InvalidArguments(format!("{path} must be an object")))?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = required_names(schema).map_err(|error| match error {
        Error::UnsupportedSchema(message) => Error::InvalidArguments(message),
        other => other,
    })?;

    for name in required {
        if !fields.contains_key(name) {
            return Err(Error::InvalidArguments(format!(
                "{path}.{name} is required"
            )));
        }
    }
    for (name, value) in fields {
        match properties.get(name) {
            Some(child) => validate_schema(child, value, &format!("{path}.{name}"))?,
            None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                return Err(Error::InvalidArguments(format!(
                    "{path}.{name} is not allowed"
                )));
            }
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar() -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create an event.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Title"},
                        "visibility": {"type": "string", "enum": ["public", "private"]},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "meta": {"type": "object", "properties": {"room": {"type": "integer"}}, "required": ["room"], "additionalProperties": false}
                    },
                    "required": ["title"],
                    "additionalProperties": false
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn round_trip_preserves_schema_and_multiple_calls() {
        let tool = calendar();
        let compact = encode_tools(&[tool.clone()]).unwrap();
        assert!(compact.prompt.contains("title:str[Title]"));
        assert_eq!(decode_tools(&compact).unwrap(), vec![tool]);
        let calls = decode_calls(
            "I can do that. <<call create_calendar_event {\"title\":\"Review\",\"attendees\":[\"a@example.com\"]}>> Done. <<call create_calendar_event {\"title\":\"Retro\",\"visibility\":\"private\"}>>",
            &decode_tools(&compact).unwrap(),
        )
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["title"], "Review");
    }

    #[test]
    fn stream_handles_split_marker_and_closing_marker_in_json() {
        let tool = calendar();
        let mut decoder = StreamDecoder::new(&[tool]);
        for chunk in [
            "answer <<ca",
            "ll create_calendar_event {\"title\":\"a >> b\"}",
            ">>",
        ] {
            decoder.push(chunk);
        }
        let calls = decoder.finish().unwrap();
        assert_eq!(calls[0].arguments["title"], "a >> b");
    }

    #[test]
    fn invalid_and_unknown_calls_fail_closed() {
        let tool = calendar();
        assert_eq!(
            decode_calls("<<call nope {\"title\":\"x\"}>>", &[tool.clone()])
                .unwrap_err()
                .code(),
            "unknown_tool"
        );
        assert_eq!(
            decode_calls(
                "<<call create_calendar_event {\"visibility\":\"secret\"}>>",
                &[tool.clone()]
            )
            .unwrap_err()
            .code(),
            "invalid_arguments"
        );
        assert_eq!(
            decode_calls(
                "<<call create_calendar_event {\"title\":\"unterminated\"}",
                &[tool]
            )
            .unwrap_err()
            .code(),
            "malformed_output"
        );
    }

    #[test]
    fn plain_answers_are_not_calls() {
        assert!(
            decode_calls("The weather is sunny.", &[calendar()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unsupported_schema_is_rejected_for_native_bypass() {
        let mut tool = calendar();
        tool.function.parameters.as_mut().unwrap()["oneOf"] = json!([]);
        assert_eq!(
            encode_tools(&[tool]).unwrap_err().code(),
            "unsupported_schema"
        );
    }

    #[test]
    fn strict_schema_rejects_invalid_types_nested_values_and_extra_fields() {
        let tool = calendar();
        let invalid_calls = [
            // Missing required root property.
            "<<call create_calendar_event {}>>",
            // String, array item, and nested integer type violations.
            "<<call create_calendar_event {\"title\":7}>>",
            "<<call create_calendar_event {\"title\":\"Review\",\"attendees\":[7]}>>",
            "<<call create_calendar_event {\"title\":\"Review\",\"meta\":{\"room\":\"12\"}}>>",
            // Missing nested required property and extra root/nested properties.
            "<<call create_calendar_event {\"title\":\"Review\",\"meta\":{}}>>",
            "<<call create_calendar_event {\"title\":\"Review\",\"unexpected\":true}>>",
            "<<call create_calendar_event {\"title\":\"Review\",\"meta\":{\"room\":12,\"floor\":3}}>>",
        ];
        for text in invalid_calls {
            assert_eq!(
                decode_calls(text, &[tool.clone()]).unwrap_err().code(),
                "invalid_arguments",
                "{text}"
            );
        }
    }

    #[test]
    fn malformed_call_syntax_never_becomes_a_tool_call() {
        let tool = calendar();
        let malformed = [
            "<<callcreate_calendar_event {\"title\":\"Review\"}>>",
            "<<call 7calendar {\"title\":\"Review\"}>>",
            "<<call create_calendar_event [\"title\"]>>",
            "<<call create_calendar_event {\"title\":\"Review\"} trailing>>",
            "<<call create_calendar_event {\"title\":\"Review\"}",
        ];
        for text in malformed {
            assert_eq!(
                decode_calls(text, &[tool.clone()]).unwrap_err().code(),
                "malformed_output",
                "{text}"
            );
        }
    }

    #[test]
    fn permissive_schema_allows_additional_properties_when_original_schema_does() {
        let mut tool = calendar();
        tool.function.parameters.as_mut().unwrap()["additionalProperties"] = json!(true);
        let calls = decode_calls(
            "<<call create_calendar_event {\"title\":\"Review\",\"venue\":\"Room 7\"}>>",
            &[tool],
        )
        .unwrap();
        assert_eq!(calls[0].arguments["venue"], "Room 7");
    }

    #[test]
    fn unsupported_constraints_and_undeclared_required_fields_bypass_compaction() {
        let mut constrained = calendar();
        constrained.function.parameters.as_mut().unwrap()["properties"]["title"]["minLength"] =
            json!(1);
        assert_eq!(
            encode_tools(&[constrained]).unwrap_err().code(),
            "unsupported_schema"
        );

        let mut undeclared_required = calendar();
        undeclared_required.function.parameters.as_mut().unwrap()["required"] =
            json!(["title", "not_in_properties"]);
        assert_eq!(
            encode_tools(&[undeclared_required]).unwrap_err().code(),
            "unsupported_schema"
        );
    }

    #[test]
    fn invalid_tool_definitions_bypass_compaction() {
        let tool = calendar();
        assert_eq!(
            encode_tools(&[tool.clone(), tool.clone()])
                .unwrap_err()
                .code(),
            "unsupported_schema"
        );

        let mut non_function = tool.clone();
        non_function.kind = "custom".into();
        assert_eq!(
            encode_tools(&[non_function]).unwrap_err().code(),
            "unsupported_schema"
        );

        let mut invalid_name = tool.clone();
        invalid_name.function.name = "calendar event".into();
        assert_eq!(
            encode_tools(&[invalid_name]).unwrap_err().code(),
            "unsupported_schema"
        );

        let mut reference_schema = tool;
        reference_schema.function.parameters.as_mut().unwrap()["properties"]["title"]["$ref"] =
            json!("#/$defs/title");
        assert_eq!(
            encode_tools(&[reference_schema]).unwrap_err().code(),
            "unsupported_schema"
        );
    }

    #[test]
    fn nested_object_and_array_arguments_round_trip_without_alteration() {
        let tool = calendar();
        let text = "<<call create_calendar_event {\"title\":\"Review\",\"attendees\":[\"a@example.com\",\"b@example.com\"],\"meta\":{\"room\":42}}>>";
        let calls = decode_calls(text, &[tool]).unwrap();
        assert_eq!(
            calls[0].arguments,
            json!({
                "title": "Review",
                "attendees": ["a@example.com", "b@example.com"],
                "meta": {"room": 42},
            })
        );
    }

    /// Property-style coverage without a random test dependency: every generated
    /// JSON string below exercises both escaped quotes/backslashes and `>>` while
    /// every chunk boundary verifies the incremental decoder's result is invariant.
    #[test]
    fn decoding_is_invariant_to_every_stream_split() {
        let tool = calendar();
        let titles = ["plain", r#"quote: \""#, "marker >> inside", r"slashes \\"];
        for title in titles {
            let text = format!(
                "prefix <<call create_calendar_event {}>> suffix",
                serde_json::json!({"title": title})
            );
            let expected = decode_calls(&text, &[tool.clone()]).unwrap();
            for split in 0..=text.len() {
                if !text.is_char_boundary(split) {
                    continue;
                }
                let mut decoder = StreamDecoder::new(&[tool.clone()]);
                decoder.push(&text[..split]);
                decoder.push(&text[split..]);
                assert_eq!(
                    decoder.finish().unwrap(),
                    expected,
                    "split={split}, title={title}"
                );
            }
        }
    }

    #[test]
    fn decoding_unicode_json_is_invariant_to_every_valid_stream_split() {
        let tool = calendar();
        let text = format!(
            "<<call create_calendar_event {}>>",
            serde_json::json!({"title": "Café ☕ — design review"})
        );
        let expected = decode_calls(&text, &[tool.clone()]).unwrap();
        for split in 0..=text.len() {
            if !text.is_char_boundary(split) {
                continue;
            }
            let mut decoder = StreamDecoder::new(&[tool.clone()]);
            decoder.push(&text[..split]);
            decoder.push(&text[split..]);
            assert_eq!(decoder.finish().unwrap(), expected, "split={split}");
        }
    }
}
