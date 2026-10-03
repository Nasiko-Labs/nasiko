use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

const CALL_START: &str = "<<call ";
const CALL_END: &str = ">>";

/// A tool definition independent of llm-router.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// A decoded compact tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// Compact textual representation of tool definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

impl CompactTools {
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CompactError {
    #[error("unknown_tool")]
    UnknownTool,

    #[error("invalid_arguments")]
    InvalidArguments,

    #[error("malformed_call")]
    MalformedCall,

    #[error("unsupported_schema")]
    UnsupportedSchema,

    #[error("incomplete_call")]
    IncompleteCall,
}

/// Convert tool definitions to a compact textual representation.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut output = String::new();

    for tool in tools {
        if tool.name.trim().is_empty() {
            return Err(CompactError::UnsupportedSchema);
        }

        output.push_str(&tool.name);
        output.push('(');

        if let Some(parameters) = &tool.parameters {
            render_properties(parameters, &mut output)?;
        }

        output.push(')');

        if let Some(description) = &tool.description {
            let description = description.trim();

            if !description.is_empty() {
                output.push_str(" - ");
                output.push_str(description);
            }
        }

        output.push('\n');
    }

    output.push_str("\nCall tools using exactly: <<call name {\"arg\":\"value\"}>>");

    Ok(CompactTools { text: output })
}

fn render_properties(schema: &Value, output: &mut String) -> Result<(), CompactError> {
    let object = schema.as_object().ok_or(CompactError::UnsupportedSchema)?;

    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(CompactError::UnsupportedSchema)?;

    let required_names: Vec<&str> = match object.get("required") {
        Some(value) => {
            let array = value.as_array().ok_or(CompactError::UnsupportedSchema)?;

            let mut names = Vec::new();

            for item in array {
                let name = item.as_str().ok_or(CompactError::UnsupportedSchema)?;

                names.push(name);
            }

            names
        }
        None => Vec::new(),
    };

    let mut first = true;

    for (name, property) in properties {
        if !first {
            output.push_str(", ");
        }

        first = false;

        output.push_str(name);

        if !required_names.contains(&name.as_str()) {
            output.push('?');
        }

        output.push(':');

        render_type(property, output)?;
    }

    Ok(())
}

fn render_type(schema: &Value, output: &mut String) -> Result<(), CompactError> {
    let object = schema.as_object().ok_or(CompactError::UnsupportedSchema)?;

    if let Some(enum_values) = object.get("enum") {
        render_enum(enum_values, output)?;
        return Ok(());
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") => output.push_str("str"),

        Some("integer") => output.push_str("int"),

        Some("number") => output.push_str("number"),

        Some("boolean") => output.push_str("bool"),

        Some("null") => output.push_str("null"),

        Some("array") => {
            output.push('[');

            match object.get("items") {
                Some(items) => render_type(items, output)?,
                None => output.push_str("any"),
            }

            output.push(']');
        }

        Some("object") => output.push_str("object"),

        _ => return Err(CompactError::UnsupportedSchema),
    }

    Ok(())
}

fn render_enum(values: &Value, output: &mut String) -> Result<(), CompactError> {
    let values = values.as_array().ok_or(CompactError::UnsupportedSchema)?;

    if values.is_empty() {
        return Err(CompactError::UnsupportedSchema);
    }

    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push('|');
        }

        match value {
            Value::String(value) => output.push_str(value),

            Value::Number(value) => {
                output.push_str(&value.to_string());
            }

            Value::Bool(value) => {
                output.push_str(if *value { "true" } else { "false" });
            }

            _ => return Err(CompactError::UnsupportedSchema),
        }
    }

    Ok(())
}

/// Decode all compact tool calls contained in arbitrary text.
///
/// Text that contains no call marker returns an empty vector.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let tool_map: HashMap<&str, &ToolDef> = tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();

    let mut calls = Vec::new();
    let mut position = 0;

    while position < text.len() {
        let Some(relative) = text[position..].find(CALL_START) else {
            break;
        };

        let start = position + relative;

        let parsed = parse_call(&text[start..], &tool_map)?;

        calls.push(parsed.call);

        position = start + parsed.consumed;
    }

    Ok(calls)
}

struct ParsedCall {
    call: ToolCall,
    consumed: usize,
}

fn parse_call(text: &str, tools: &HashMap<&str, &ToolDef>) -> Result<ParsedCall, CompactError> {
    if !text.starts_with(CALL_START) {
        return Err(CompactError::MalformedCall);
    }

    let after_marker = &text[CALL_START.len()..];

    let space = after_marker.find(' ').ok_or(CompactError::MalformedCall)?;

    let name = &after_marker[..space];

    if name.is_empty() {
        return Err(CompactError::MalformedCall);
    }

    let tool = tools.get(name).ok_or(CompactError::UnknownTool)?;

    let json_start = CALL_START.len() + space + 1;

    let json_text = &text[json_start..];

    let json_length = find_json_object_end(json_text).ok_or(CompactError::IncompleteCall)?;

    let arguments = &json_text[..json_length];

    let end_of_json = json_start + json_length;

    if !text[end_of_json..].starts_with(CALL_END) {
        return Err(CompactError::IncompleteCall);
    }

    let value: Value =
        serde_json::from_str(arguments).map_err(|_| CompactError::InvalidArguments)?;

    validate_schema(tool.parameters.as_ref(), &value)?;

    let consumed = end_of_json + CALL_END.len();

    Ok(ParsedCall {
        call: ToolCall {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
        consumed,
    })
}

/// Finds the end of a JSON object.
///
/// This parser understands nested objects, arrays, strings,
/// escaped characters, and therefore does not mistake `>>`
/// inside a JSON string for the call terminator.
fn find_json_object_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();

    if bytes.first() != Some(&b'{') {
        return None;
    }

    let mut object_depth = 0usize;
    let mut array_depth = 0usize;

    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }

            continue;
        }

        match byte {
            b'"' => {
                in_string = true;
            }

            b'{' => {
                object_depth += 1;
            }

            b'}' => {
                if object_depth == 0 {
                    return None;
                }

                object_depth -= 1;

                if object_depth == 0 && array_depth == 0 {
                    return Some(index + 1);
                }
            }

            b'[' => {
                array_depth += 1;
            }

            b']' => {
                if array_depth == 0 {
                    return None;
                }

                array_depth -= 1;
            }

            _ => {}
        }
    }

    None
}

/// Validate the supported JSON Schema subset.
fn validate_schema(schema: Option<&Value>, value: &Value) -> Result<(), CompactError> {
    let Some(schema) = schema else {
        return Ok(());
    };

    validate_value(schema, value)
}

fn validate_value(schema: &Value, value: &Value) -> Result<(), CompactError> {
    let object = schema.as_object().ok_or(CompactError::UnsupportedSchema)?;

    // enum validation
    if let Some(enum_values) = object.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or(CompactError::UnsupportedSchema)?;

        if !values.iter().any(|candidate| candidate == value) {
            return Err(CompactError::InvalidArguments);
        }
    }

    let schema_type = object.get("type").and_then(Value::as_str);

    // object validation
    if schema_type == Some("object") {
        let actual = value.as_object().ok_or(CompactError::InvalidArguments)?;

        if let Some(required) = object.get("required") {
            let required = required.as_array().ok_or(CompactError::UnsupportedSchema)?;

            for required_value in required {
                let name = required_value
                    .as_str()
                    .ok_or(CompactError::UnsupportedSchema)?;

                if !actual.contains_key(name) {
                    return Err(CompactError::InvalidArguments);
                }
            }
        }

        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            for (name, property_schema) in properties {
                if let Some(actual_value) = actual.get(name) {
                    validate_value(property_schema, actual_value)?;
                }
            }
        }

        return Ok(());
    }

    // array validation
    if schema_type == Some("array") {
        let actual = value.as_array().ok_or(CompactError::InvalidArguments)?;

        if let Some(item_schema) = object.get("items") {
            for item in actual {
                validate_value(item_schema, item)?;
            }
        }

        return Ok(());
    }

    match schema_type {
        Some("string") => {
            if value.is_string() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments)
            }
        }

        Some("integer") => {
            if value.is_i64() || value.is_u64() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments)
            }
        }

        Some("number") => {
            if value.is_number() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments)
            }
        }

        Some("boolean") => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments)
            }
        }

        Some("null") => {
            if value.is_null() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments)
            }
        }

        Some(_) => Err(CompactError::UnsupportedSchema),

        None => {
            if object.contains_key("enum") {
                Ok(())
            } else {
                Err(CompactError::UnsupportedSchema)
            }
        }
    }
}

/// Streaming decoder.
///
/// It accepts arbitrary chunks, including chunks that split
/// the call marker or JSON arguments.
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

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        self.buffer.push_str(chunk);

        match decode_calls(&self.buffer, &self.tools) {
            Ok(calls) => {
                if !calls.is_empty() {
                    self.buffer.clear();
                }

                Ok(calls)
            }

            Err(CompactError::IncompleteCall) => Ok(Vec::new()),

            Err(error) => Err(error),
        }
    }

    pub fn finish(&self) -> Result<Vec<ToolCall>, CompactError> {
        decode_calls(&self.buffer, &self.tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string"
                    },
                    "start": {
                        "type": "string"
                    },
                    "duration_min": {
                        "type": "integer"
                    },
                    "attendees": {
                        "type": "array",
                        "items": {
                            "type": "string"
                        }
                    },
                    "visibility": {
                        "type": "string",
                        "enum": [
                            "public",
                            "private"
                        ]
                    }
                },
                "required": [
                    "title",
                    "start"
                ]
            })),
        }
    }

    #[test]
    fn encodes_compact_schema() {
        let result = encode_tools(&[calendar_tool()]).unwrap();

        assert!(result.text.contains("create_calendar_event("));

        assert!(result.text.contains("title:str"));

        assert!(result.text.contains("start:str"));

        assert!(result.text.contains("duration_min?:int"));

        assert!(result.text.contains("attendees?:[str]"));

        assert!(result.text.contains("visibility?:public|private"));

        assert!(result.text.contains("<<call name"));
    }

    #[test]
    fn decodes_call() {
        let tools = vec![calendar_tool()];

        let text = r#"Hello
<<call create_calendar_event {"title":"Design review","start":"2026-10-03T10:00:00"}>>
Done"#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);

        assert_eq!(calls[0].name, "create_calendar_event");

        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();

        assert_eq!(args["title"], "Design review");
    }

    #[test]
    fn plain_text_has_no_calls() {
        let tools = vec![calendar_tool()];

        let calls = decode_calls("The weather is sunny.", &tools).unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn multiple_calls_are_decoded() {
        let tools = vec![calendar_tool()];

        let text = concat!(
            "<<call create_calendar_event ",
            r#"{"title":"A","start":"2026-10-03"}"#,
            ">>\n",
            "<<call create_calendar_event ",
            r#"{"title":"B","start":"2026-10-04"}"#,
            ">>"
        );

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn unknown_tool_is_rejected() {
        let tools = vec![calendar_tool()];

        let result = decode_calls(r#"<<call delete_everything {}>>"#, &tools);

        assert_eq!(result, Err(CompactError::UnknownTool));
    }

    #[test]
    fn missing_required_argument_is_rejected() {
        let tools = vec![calendar_tool()];

        let result = decode_calls(
            r#"<<call create_calendar_event {"start":"2026-10-03"}>>"#,
            &tools,
        );

        assert_eq!(result, Err(CompactError::InvalidArguments));
    }

    #[test]
    fn wrong_type_is_rejected() {
        let tools = vec![calendar_tool()];

        let result = decode_calls(
            r#"<<call create_calendar_event {"title":123,"start":"2026-10-03"}>>"#,
            &tools,
        );

        assert_eq!(result, Err(CompactError::InvalidArguments));
    }

    #[test]
    fn invalid_enum_is_rejected() {
        let tools = vec![calendar_tool()];

        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"x","start":"y","visibility":"secret"}>>"#,
            &tools,
        );

        assert_eq!(result, Err(CompactError::InvalidArguments));
    }

    #[test]
    fn marker_inside_string_is_supported() {
        let tools = vec![calendar_tool()];

        let text = r#"<<call create_calendar_event {"title":"A >> B","start":"2026-10-03"}>>"#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn nested_objects_are_supported() {
        let tool = ToolDef {
            name: "configure".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "config": {
                        "type": "object",
                        "properties": {
                            "enabled": {
                                "type": "boolean"
                            }
                        },
                        "required": [
                            "enabled"
                        ]
                    }
                },
                "required": [
                    "config"
                ]
            })),
        };

        let text = r#"<<call configure {"config":{"enabled":true}}>>"#;

        let calls = decode_calls(text, &[tool]).unwrap();

        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn split_marker_stream_is_supported() {
        let tools = vec![calendar_tool()];

        let mut decoder = StreamDecoder::new(tools);

        let first = decoder.push("Hello <<cal").unwrap();

        assert!(first.is_empty());

        let second = decoder
            .push(r#"l create_calendar_event {"title":"A","start":"2026-10-03"}>>"#)
            .unwrap();

        assert_eq!(second.len(), 1);

        assert_eq!(second[0].name, "create_calendar_event");
    }

    #[test]
    fn split_json_stream_is_supported() {
        let tools = vec![calendar_tool()];

        let mut decoder = StreamDecoder::new(tools);

        let first = decoder
            .push(r#"<<call create_calendar_event {"title":"A","#)
            .unwrap();

        assert!(first.is_empty());

        let second = decoder.push(r#""start":"2026-10-03"}>>"#).unwrap();

        assert_eq!(second.len(), 1);
    }

    #[test]
    fn malformed_incomplete_call_is_detected() {
        let tools = vec![calendar_tool()];

        let result = decode_calls(r#"<<call create_calendar_event {"title":"x"}"#, &tools);

        assert_eq!(result, Err(CompactError::IncompleteCall));
    }
}
