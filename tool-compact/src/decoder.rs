use crate::error::{Error, Result};
use crate::types::{ToolCall, ToolDef};
use serde_json::Value;

/// Decode compact tool calls from model output.
///
/// Example:
/// `<<call get_weather {"city":"Hyderabad"}>>`
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find("<<call") {
        let start = cursor + relative_start;

        let end = find_call_end(&text[start..]).ok_or(Error::InvalidSyntax)?;

        let call_text = &text[start..start + end];

        let call = parse_call(call_text, tools)?;
        calls.push(call);

        cursor = start + end;
    }

    Ok(calls)
}

/// Streaming decoder for compact tool calls.
///
/// Input may arrive in arbitrary chunks. The decoder buffers incomplete
/// calls until a complete `>>` marker is received.
pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    buffer: String,
    calls: Vec<ToolCall>,
}

impl<'a> StreamDecoder<'a> {
    /// Create a new stream decoder.
    pub fn new(tools: &'a [ToolDef]) -> Self {
        Self {
            tools,
            buffer: String::new(),
            calls: Vec::new(),
        }
    }

    /// Push the next chunk of model output.
    ///
    /// Complete calls are decoded immediately. Incomplete data remains
    /// buffered until another chunk is pushed.
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        self.buffer.push_str(chunk);

        self.process_buffer()
    }

    /// Finish the stream.
    ///
    /// Returns all successfully decoded calls. If an incomplete compact
    /// call remains in the buffer, the stream fails closed.
    pub fn finish(mut self) -> Result<Vec<ToolCall>> {
        self.process_buffer()?;

        if self.buffer.contains("<<call") {
            return Err(Error::InvalidSyntax);
        }

        Ok(self.calls)
    }

    /// Return calls decoded so far without consuming the decoder.
    pub fn calls(&self) -> &[ToolCall] {
        &self.calls
    }

    fn process_buffer(&mut self) -> Result<()> {
        loop {
            let Some(start) = self.buffer.find("<<call") else {
                // No beginning of a call remains.
                //
                // Keep only a small suffix because `<<call` itself may
                // be split across chunks.
                self.retain_marker_suffix();
                return Ok(());
            };

            // Discard ordinary model text before the call.
            if start > 0 {
                self.buffer.drain(..start);
            }

            let Some(end) = find_call_end(&self.buffer) else {
                // The call is incomplete. Keep it for the next chunk.
                return Ok(());
            };

            let call_text = self.buffer[..end].to_string();

            let call = parse_call(&call_text, self.tools)?;

            self.calls.push(call);

            self.buffer.drain(..end);
        }
    }

    fn retain_marker_suffix(&mut self) {
        const MARKER: &str = "<<call";

        let max_suffix = MARKER.len().saturating_sub(1);

        let keep = self
            .buffer
            .char_indices()
            .rev()
            .take(max_suffix)
            .last()
            .map(|(index, _)| index)
            .unwrap_or(self.buffer.len());

        if self.buffer.len() > max_suffix {
            let split = self.buffer.len() - max_suffix;
            self.buffer.drain(..split);
        } else {
            let _ = keep;
        }
    }
}

/// Find the end of a compact call while respecting JSON strings.
///
/// This prevents `>>` inside a JSON string from terminating the call.
fn find_call_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();

    let mut in_string = false;
    let mut escaped = false;

    let mut i = 0;

    while i + 1 < bytes.len() {
        let byte = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if bytes[i] == b'>' && bytes[i + 1] == b'>' {
            return Some(i + 2);
        }

        i += 1;
    }

    None
}

fn parse_call(text: &str, tools: &[ToolDef]) -> Result<ToolCall> {
    let inner = text
        .strip_prefix("<<call")
        .ok_or(Error::InvalidSyntax)?
        .strip_suffix(">>")
        .ok_or(Error::InvalidSyntax)?
        .trim();

    if inner.is_empty() {
        return Err(Error::InvalidToolCall("missing tool name".to_string()));
    }

    let mut parts = inner.splitn(2, char::is_whitespace);

    let name = parts
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::InvalidToolCall("missing tool name".to_string()))?;

    let arguments_text = parts
        .next()
        .map(str::trim)
        .filter(|args| !args.is_empty())
        .ok_or_else(|| Error::InvalidToolCall(format!("missing arguments for tool `{name}`")))?;

    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

    let arguments: Value = serde_json::from_str(arguments_text)?;

    if !arguments.is_object() {
        return Err(Error::InvalidArguments(format!(
            "arguments for `{name}` must be a JSON object"
        )));
    }

    validate_arguments(tool, &arguments)?;

    Ok(ToolCall {
        name: name.to_string(),
        arguments,
    })
}

fn validate_arguments(tool: &ToolDef, arguments: &Value) -> Result<()> {
    let object = arguments.as_object().ok_or_else(|| {
        Error::InvalidArguments(format!("arguments for `{}` must be an object", tool.name))
    })?;

    let Some(schema) = &tool.parameters else {
        return Ok(());
    };

    let Some(schema_object) = schema.as_object() else {
        return Err(Error::InvalidArguments(format!(
            "invalid parameter schema for `{}`",
            tool.name
        )));
    };

    let Some(properties) = schema_object.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };

    if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
        for required_name in required {
            let Some(name) = required_name.as_str() else {
                return Err(Error::InvalidArguments(format!(
                    "invalid required field definition for `{}`",
                    tool.name
                )));
            };

            if !object.contains_key(name) {
                return Err(Error::InvalidArguments(format!(
                    "missing required argument `{name}`"
                )));
            }
        }
    }

    for (name, value) in object {
        let property_schema = properties
            .get(name)
            .ok_or_else(|| Error::InvalidArguments(format!("unknown argument `{name}`")))?;

        validate_value(name, value, property_schema)?;
    }

    Ok(())
}

fn validate_value(name: &str, value: &Value, schema: &Value) -> Result<()> {
    let schema_object = schema
        .as_object()
        .ok_or_else(|| Error::InvalidArguments(format!("invalid schema for argument `{name}`")))?;

    if let Some(enum_values) = schema_object.get("enum") {
        let Some(enum_values) = enum_values.as_array() else {
            return Err(Error::InvalidArguments(format!(
                "invalid enum for argument `{name}`"
            )));
        };

        if !enum_values.iter().any(|allowed| allowed == value) {
            return Err(Error::InvalidArguments(format!(
                "invalid enum value for argument `{name}`"
            )));
        }
    }

    let expected_type = schema_object.get("type").and_then(Value::as_str);

    match expected_type {
        Some("object") => {
            let object = value.as_object().ok_or_else(|| {
                Error::InvalidArguments(format!("argument `{name}` must be an object"))
            })?;

            if let Some(properties) = schema_object.get("properties").and_then(Value::as_object) {
                if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
                    for required_name in required {
                        let required_name = required_name.as_str().ok_or_else(|| {
                            Error::InvalidArguments(format!("invalid required field in `{name}`"))
                        })?;

                        if !object.contains_key(required_name) {
                            return Err(Error::InvalidArguments(format!(
                                "missing required nested argument `{required_name}`"
                            )));
                        }
                    }
                }

                for (child_name, child_value) in object {
                    let child_schema = properties.get(child_name).ok_or_else(|| {
                        Error::InvalidArguments(format!("unknown nested argument `{child_name}`"))
                    })?;

                    validate_value(child_name, child_value, child_schema)?;
                }
            }
        }

        Some("string") => {
            if !value.is_string() {
                return Err(Error::InvalidArguments(format!(
                    "argument `{name}` must be a string"
                )));
            }
        }

        Some("integer") => {
            if !value.is_i64() && !value.is_u64() {
                return Err(Error::InvalidArguments(format!(
                    "argument `{name}` must be an integer"
                )));
            }
        }

        Some("number") => {
            if !value.is_number() {
                return Err(Error::InvalidArguments(format!(
                    "argument `{name}` must be a number"
                )));
            }
        }

        Some("boolean") => {
            if !value.is_boolean() {
                return Err(Error::InvalidArguments(format!(
                    "argument `{name}` must be a boolean"
                )));
            }
        }

        Some("array") => {
            let array = value.as_array().ok_or_else(|| {
                Error::InvalidArguments(format!("argument `{name}` must be an array"))
            })?;

            if let Some(items_schema) = schema_object.get("items") {
                for item in array {
                    validate_value(name, item, items_schema)?;
                }
            }
        }

        Some(other) => {
            return Err(Error::InvalidArguments(format!(
                "unsupported type `{other}` for argument `{name}`"
            )));
        }

        None => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn weather_tool() -> ToolDef {
        ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get weather".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    }
                },
                "required": ["city"]
            })),
        }
    }

    fn mode_tool() -> ToolDef {
        ToolDef {
            name: "set_mode".to_string(),
            description: Some("Set mode".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["fast", "safe"]
                    }
                },
                "required": ["mode"]
            })),
        }
    }

    #[test]
    fn no_calls_returns_empty_vec() {
        let tools = vec![weather_tool()];

        let result = decode_calls("hello world", &tools).unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn surrounding_text_is_ignored() {
        let tools = vec![weather_tool()];

        let result = decode_calls(
            "Sure, I'll help. <<call get_weather {\"city\":\"Hyderabad\"}>> Done.",
            &tools,
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_weather");
    }

    #[test]
    fn multiple_calls_with_surrounding_text() {
        let tools = vec![weather_tool(), mode_tool()];

        let text = r#"
            First:
            <<call get_weather {"city":"Hyderabad"}>>

            Then:
            <<call set_mode {"mode":"fast"}>>
        "#;

        let result = decode_calls(text, &tools).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "get_weather");
        assert_eq!(result[1].name, "set_mode");
    }

    #[test]
    fn empty_call_is_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls("<<call>>", &tools);

        assert!(result.is_err());
    }

    #[test]
    fn missing_arguments_are_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls("<<call get_weather>>", &tools);

        assert!(result.is_err());
    }

    #[test]
    fn empty_arguments_are_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls("<<call get_weather   >>", &tools);

        assert!(result.is_err());
    }

    #[test]
    fn invalid_json_is_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls(r#"<<call get_weather {"city":}"#, &tools);

        assert!(result.is_err());
    }

    #[test]
    fn non_object_arguments_are_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls(r#"<<call get_weather ["Hyderabad"]>>"#, &tools);

        assert!(result.is_err());
    }

    #[test]
    fn invalid_call_among_valid_calls_fails_closed() {
        let tools = vec![weather_tool(), mode_tool()];

        let text = r#"
            <<call get_weather {"city":"Hyderabad"}>>
            <<call set_mode {"mode":"invalid"}>>
        "#;

        let result = decode_calls(text, &tools);

        assert!(result.is_err());
    }

    #[test]
    fn malformed_marker_is_rejected() {
        let tools = vec![weather_tool()];

        let result = decode_calls(r#"<<call get_weather {"city":"Hyderabad"}"#, &tools);

        assert!(result.is_err());
    }

    #[test]
    fn whitespace_is_allowed() {
        let tools = vec![weather_tool()];

        let result = decode_calls(
            "  <<call   get_weather   {\"city\":\"Hyderabad\"}  >>  ",
            &tools,
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_weather");
    }

    #[test]
    fn unknown_tool_still_fails() {
        let tools = vec![weather_tool()];

        let result = decode_calls(r#"<<call get_time {"city":"Hyderabad"}>>"#, &tools);

        assert!(matches!(result, Err(Error::UnknownTool(_))));
    }

    #[test]
    fn string_containing_end_marker_is_supported() {
        let tool = ToolDef {
            name: "echo".to_string(),
            description: Some("Echo text".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string"
                    }
                },
                "required": ["text"]
            })),
        };

        let result = decode_calls(r#"<<call echo {"text":"hello >> world"}>>"#, &[tool]).unwrap();

        assert_eq!(result[0].arguments["text"], "hello >> world");
    }

    #[test]
    fn stream_handles_split_call() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder.push("<<call get_wea").unwrap();
        decoder.push("ther {\"city\":\"Hyd").unwrap();
        decoder.push("erabad\"}>>").unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments["city"], "Hyderabad");
    }

    #[test]
    fn stream_handles_split_end_marker() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"<<call get_weather {"city":"Hyderabad"}>"#)
            .unwrap();

        assert!(decoder.calls().is_empty());

        decoder.push(">").unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn stream_handles_multiple_calls() {
        let tools = vec![weather_tool(), mode_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"<<call get_weather {"city":"Hyderabad"}>>"#)
            .unwrap();

        decoder
            .push(r#"<<call set_mode {"mode":"fast"}>>"#)
            .unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].name, "set_mode");
    }

    #[test]
    fn stream_handles_text_before_call() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push("I'll check that. <<call get_weather ")
            .unwrap();

        decoder.push(r#"{"city":"Hyderabad"}>>"#).unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn stream_handles_end_marker_inside_string() {
        let tool = ToolDef {
            name: "echo".to_string(),
            description: Some("Echo text".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string"
                    }
                },
                "required": ["text"]
            })),
        };

        let tools = [tool];
        let mut decoder = StreamDecoder::new(&tools);

        decoder.push(r#"<<call echo {"text":"hello >>"#).unwrap();

        decoder.push(r#" world"}>>"#).unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["text"], "hello >> world");
    }

    #[test]
    fn stream_rejects_incomplete_call_on_finish() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"<<call get_weather {"city":"Hyderabad"}"#)
            .unwrap();

        assert!(decoder.finish().is_err());
    }

    #[test]
    fn stream_returns_no_calls_for_plain_text() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder.push("hello world").unwrap();

        let calls = decoder.finish().unwrap();

        assert!(calls.is_empty());
    }

    #[test]
    fn stream_handles_call_marker_split_across_chunks() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder.push("<").unwrap();
        decoder.push("<cal").unwrap();
        decoder.push("l get_weather ").unwrap();
        decoder.push(r#"{"city":"Hyderabad"}"#).unwrap();
        decoder.push(">>").unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments["city"], "Hyderabad");
    }

    #[test]
    fn stream_handles_marker_split_one_character_at_a_time() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        let chunks = [
            "<",
            "<",
            "c",
            "a",
            "l",
            "l",
            " ",
            "get_weather",
            " ",
            r#"{"city":"Hyderabad"}"#,
            ">",
            ">",
        ];

        for chunk in chunks {
            decoder.push(chunk).unwrap();
        }

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn stream_handles_second_call_split_across_chunks() {
        let tools = vec![weather_tool(), mode_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"<<call get_weather {"city":"Hyderabad"}>>"#)
            .unwrap();

        decoder.push("<<call set_").unwrap();
        decoder.push(r#"mode {"mode":"fast"}>>"#).unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].name, "set_mode");
    }

    #[test]
    fn stream_handles_text_between_calls() {
        let tools = vec![weather_tool(), mode_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"First: <<call get_weather {"city":"Hyderabad"}>>"#)
            .unwrap();

        decoder.push("Some text between calls. ").unwrap();

        decoder
            .push(r#"Second: <<call set_mode {"mode":"safe"}>>"#)
            .unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].name, "set_mode");
    }

    #[test]
    fn stream_rejects_invalid_call_after_valid_call() {
        let tools = vec![weather_tool(), mode_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder
            .push(r#"<<call get_weather {"city":"Hyderabad"}>>"#)
            .unwrap();

        let result = decoder.push(r#"<<call set_mode {"mode":"invalid"}>>"#);

        assert!(result.is_err());
    }

    #[test]
    fn stream_rejects_unknown_tool() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        let result = decoder.push(r#"<<call get_time {"city":"Hyderabad"}>>"#);

        assert!(matches!(result, Err(Error::UnknownTool(_))));
    }

    #[test]
    fn stream_keeps_partial_marker_between_chunks() {
        let tools = vec![weather_tool()];
        let mut decoder = StreamDecoder::new(&tools);

        decoder.push("ordinary text <").unwrap();
        decoder.push("<call get_weather ").unwrap();
        decoder.push(r#"{"city":"Hyderabad"}>>"#).unwrap();

        let calls = decoder.finish().unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }
}
