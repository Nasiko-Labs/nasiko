use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    pub text: String,
}

impl CompactTools {
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for CompactTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("invalid call: {0}")]
    InvalidCall(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid arguments for {0}: {1}")]
    InvalidArguments(String, String),

    #[error("unsupported schema: {0}")]
    UnsupportedSchema(String),

    #[error("incomplete stream")]
    IncompleteStream,

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, CompactError>;

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut output = String::new();

    for (index, tool) in tools.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }

        output.push_str("tool(");
        output.push_str(&tool.name);

        if let Some(parameters) = &tool.parameters {
            let object = parameters
                .as_object()
                .ok_or_else(|| {
                    CompactError::UnsupportedSchema(
                        format!("tool '{}' parameters must be an object", tool.name)
                    )
                })?;

            if let Some(properties) =
                object.get("properties").and_then(Value::as_object)
            {
                output.push(' ');

                let required = object
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let mut first = true;

                for (name, schema) in properties {
                    if !first {
                        output.push(',');
                    }
                    first = false;

                    output.push_str(name);
                    output.push(':');
                    output.push_str(schema_type(schema));

                    if required.contains(&name.as_str()) {
                        output.push('!');
                    } else {
                        output.push('?');
                    }
                }
            }
        }

        output.push(')');
    }

    Ok(CompactTools { text: output })
}

fn schema_type(schema: &Value) -> &str {
    schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object")
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut position = 0;

    while let Some(relative_start) = text[position..].find("<<call ") {
        let start = position + relative_start;
        let content_start = start + "<<call ".len();

        let name_end_relative = text[content_start..]
            .find(char::is_whitespace)
            .ok_or_else(|| {
                CompactError::IncompleteStream
            })?;

        let name_end = content_start + name_end_relative;
        let name = &text[content_start..name_end];

        let json_start = text[name_end..]
            .find('{')
            .map(|offset| name_end + offset)
            .ok_or(CompactError::IncompleteStream)?;

        let remaining = &text[json_start..];

        let end = find_json_call_end(remaining)?;

        let json_text = &remaining[..end];

        let arguments: Value = serde_json::from_str(json_text)?;

        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| CompactError::UnknownTool(name.to_string()))?;

        validate_arguments(tool.parameters.as_ref(), &arguments)
            .map_err(|message| {
                CompactError::InvalidArguments(name.to_string(), message)
            })?;

        calls.push(ToolCall {
            name: name.to_string(),
            arguments,
        });

        position = json_start + end + 2;
    }

    Ok(calls)
}

fn find_json_call_end(text: &str) -> Result<usize> {
    let bytes = text.as_bytes();

    if bytes.first() != Some(&b'{') {
        return Err(CompactError::InvalidCall(
            "arguments must be a JSON object".into(),
        ));
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for i in 0..bytes.len() {
        let c = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }

            continue;
        }

        match c {
            b'"' => in_string = true,

            b'{' => {
                depth += 1;
            }

            b'}' => {
                depth = depth.saturating_sub(1);

                if depth == 0 {
                    let rest = &text[i + 1..];

                    if rest.starts_with(">>") {
                        return Ok(i + 1);
                    }

                    if rest.trim_start().starts_with(">>") {
                        let whitespace =
                            rest.len() - rest.trim_start().len();

                        return Ok(i + 1 + whitespace);
                    }
                }
            }

            _ => {}
        }
    }

    Err(CompactError::IncompleteStream)
}

fn validate_arguments(
    schema: Option<&Value>,
    arguments: &Value,
) -> std::result::Result<(), String> {
    let schema = schema.ok_or("missing schema")?;

    validate_value(schema, arguments, "$")
}

fn validate_value(
    schema: &Value,
    value: &Value,
    path: &str,
) -> std::result::Result<(), String> {
    if let Some(enum_values) = schema.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or("invalid enum definition")?;

        if !values.iter().any(|candidate| candidate == value) {
            return Err(format!(
                "{} is not an allowed enum value",
                path
            ));
        }
    }

    let schema_type = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object");

    match schema_type {
        "object" => {
            let object = value
                .as_object()
                .ok_or_else(|| {
                    format!("{} must be an object", path)
                })?;

            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .ok_or("schema has no properties")?;

            if let Some(required) = schema.get("required") {
                for item in required
                    .as_array()
                    .ok_or("required must be an array")?
                {
                    let name = item
                        .as_str()
                        .ok_or("required name must be a string")?;

                    if !object.contains_key(name) {
                        return Err(format!(
                            "{} is missing required field '{}'",
                            path, name
                        ));
                    }
                }
            }

            for (name, child_value) in object {
                if let Some(child_schema) = properties.get(name) {
                    validate_value(
                        child_schema,
                        child_value,
                        &format!("{}.{}", path, name),
                    )?;
                }
            }
        }

        "string" => {
            if !value.is_string() {
                return Err(format!("{} must be a string", path));
            }
        }

        "integer" => {
            if !value.is_i64() && !value.is_u64() {
                return Err(format!("{} must be an integer", path));
            }
        }

        "number" => {
            if !value.is_number() {
                return Err(format!("{} must be a number", path));
            }
        }

        "boolean" => {
            if !value.is_boolean() {
                return Err(format!("{} must be a boolean", path));
            }
        }

        "array" => {
            let array = value
                .as_array()
                .ok_or_else(|| {
                    format!("{} must be an array", path)
                })?;

            let item_schema = schema
                .get("items")
                .ok_or("array schema has no items")?;

            for (index, item) in array.iter().enumerate() {
                validate_value(
                    item_schema,
                    item,
                    &format!("{}[{}]", path, index),
                )?;
            }
        }

        other => {
            return Err(format!(
                "{} uses unsupported type '{}'",
                path, other
            ));
        }
    }

    Ok(())
}

/// Incremental decoder.
///
/// It safely handles markers split across stream chunks.
pub struct StreamDecoder {
    buffer: String,
    tools: Vec<ToolDef>,
    emitted: usize,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            buffer: String::new(),
            tools,
            emitted: 0,
        }
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);

        if !self.buffer.contains("<<call ") {
            return Ok(Vec::new());
        }

        if !self.buffer.contains('{') {
            return Ok(Vec::new());
        }

        match decode_calls(&self.buffer, &self.tools) {
            Ok(calls) => {
                let new_calls: Vec<ToolCall> =
                    calls.into_iter().skip(self.emitted).collect();

                self.emitted += new_calls.len();

                Ok(new_calls)
            }

            Err(CompactError::IncompleteStream) => {
                Ok(Vec::new())
            }

            Err(CompactError::InvalidCall(message))
                if message == "missing JSON arguments" =>
            {
                Ok(Vec::new())
            }

            Err(error) => Err(error),
        }
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        let calls = decode_calls(&self.buffer, &self.tools)?;

        let new_calls: Vec<ToolCall> =
            calls.into_iter().skip(self.emitted).collect();

        self.emitted += new_calls.len();

        Ok(new_calls)
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
        self.emitted = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event".into()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "duration_min": {"type": "integer"}
                    },
                    "required": ["title"]
                })),
            },
        ]
    }

    #[test]
    fn compact_encode_works() {
        let encoded = encode_tools(&tools()).unwrap();

        assert!(encoded.text.contains("create_calendar_event"));
        assert!(encoded.text.contains("title:string!"));
        assert!(encoded.text.contains("duration_min:integer?"));
    }

    #[test]
    fn decode_call_works() {
        let text =
            r#"Hello <<call create_calendar_event {"title":"Meeting","duration_min":30}>> done"#;

        let calls = decode_calls(text, &tools()).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(
            calls[0].arguments["title"],
            "Meeting"
        );
    }

    #[test]
    fn multiple_calls_work() {
        let text = r#"
            <<call create_calendar_event {"title":"A"}>>
            text
            <<call create_calendar_event {"title":"B"}>>
        "#;

        let calls = decode_calls(text, &tools()).unwrap();

        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn missing_required_field_fails() {
        let text =
            r#"<<call create_calendar_event {"duration_min":30}>>"#;

        assert!(decode_calls(text, &tools()).is_err());
    }

    #[test]
    fn unknown_tool_fails() {
        let text =
            r#"<<call unknown {"title":"test"}>>"#;

        assert!(decode_calls(text, &tools()).is_err());
    }

    #[test]
    fn stream_split_marker_works() {
        let mut decoder = StreamDecoder::new(tools());

        assert!(decoder.push("Hello <").unwrap().is_empty());

        assert!(
            decoder
                .push("<call create_calendar_event ")
                .unwrap()
                .is_empty()
        );

        let calls = decoder
            .push(r#"{"title":"Test"}>>"#)
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].name,
            "create_calendar_event"
        );
    }

    #[test]
    fn escaping_works() {
        let text =
            r#"<<call create_calendar_event {"title":"A \"quoted\" meeting"}>>"#;

        let calls = decode_calls(text, &tools()).unwrap();

        assert_eq!(
            calls[0].arguments["title"],
            r#"A "quoted" meeting"#
        );
    }
}