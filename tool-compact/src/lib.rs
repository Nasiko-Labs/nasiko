use serde_json::{json, Value};
use std::fmt;

pub type Result<T> = std::result::Result<T, CompactError>;

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct CompactTools(pub String);

#[derive(Debug, Clone, PartialEq)]
pub enum CompactError {
    InvalidFormat(String),
    UnknownTool(String),
    MissingArgument(String),
    InvalidArgument(String),
    InvalidJson(String),
}

impl fmt::Display for CompactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CompactError {}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut output = String::new();

    for tool in tools {
        output.push_str(&tool.name);

        if let Some(desc) = &tool.description {
            output.push_str(" - ");
            output.push_str(desc);
        }

        output.push('\n');

        if let Some(properties) = tool.parameters.get("properties") {
            output.push_str(&format!("schema: {}\n", properties));
        }

        output.push('\n');
    }

    Ok(CompactTools(output))
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut pos = 0;

    while let Some(start_rel) = text[pos..].find("<<call ") {
        let start = pos + start_rel;
        let after_marker = start + "<<call ".len();

        let name_end = text[after_marker..]
            .find(' ')
            .ok_or_else(|| CompactError::InvalidFormat("missing tool name".into()))?;

        let name_end = after_marker + name_end;
        let name = &text[after_marker..name_end];

        let json_start = name_end + 1;
        let json_end = find_json_end(text, json_start)?;

        let json_text = &text[json_start..=json_end];

        let arguments: Value = serde_json::from_str(json_text)
            .map_err(|e| CompactError::InvalidJson(e.to_string()))?;

        let closing = json_end + 1;
        if !text[closing..].starts_with(">>") {
            return Err(CompactError::InvalidFormat(
                "missing closing >>".into(),
            ));
        }

        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| CompactError::UnknownTool(name.to_string()))?;

        validate_arguments(tool, &arguments)?;

        calls.push(ToolCall {
            name: name.to_string(),
            arguments,
        });

        pos = closing + 2;
    }

    Ok(calls)
}

fn find_json_end(text: &str, start: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    if start >= bytes.len() || bytes[start] != b'{' {
        return Err(CompactError::InvalidFormat(
            "tool arguments must be JSON object".into(),
        ));
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for i in start..bytes.len() {
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
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }

    Err(CompactError::InvalidFormat(
        "incomplete JSON arguments".into(),
    ))
}

fn validate_arguments(tool: &ToolDef, args: &Value) -> Result<()> {
    let object = args
        .as_object()
        .ok_or_else(|| CompactError::InvalidArgument("arguments must be an object".into()))?;

    if let Some(required) = tool.parameters.get("required").and_then(Value::as_array) {
        for field in required {
            if let Some(name) = field.as_str() {
                if !object.contains_key(name) {
                    return Err(CompactError::MissingArgument(name.to_string()));
                }
            }
        }
    }

    if let Some(properties) = tool.parameters.get("properties").and_then(Value::as_object) {
        for (name, value) in object {
            if let Some(schema) = properties.get(name) {
                if let Some(enums) = schema.get("enum").and_then(Value::as_array) {
                    if !enums.iter().any(|v| v == value) {
                        return Err(CompactError::InvalidArgument(format!(
                            "invalid enum value for {name}"
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

pub struct StreamDecoder {
    buffer: String,
}

impl StreamDecoder {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);

        let mut completed = Vec::new();

        loop {
            let Some(start) = self.buffer.find("<<call ") else {
                break;
            };

            let candidate = &self.buffer[start..];

            if !candidate.contains(">>") {
                break;
            }

            match decode_calls(candidate, tools) {
                Ok(calls) => {
                    completed.extend(calls);
                    self.buffer.clear();
                    break;
                }
                Err(CompactError::InvalidFormat(_))
                | Err(CompactError::InvalidJson(_)) => {
                    break;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(completed)
    }

    pub fn finish(&mut self, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
        if self.buffer.trim().is_empty() {
            return Ok(Vec::new());
        }

        let calls = decode_calls(&self.buffer, tools)?;
        self.buffer.clear();
        Ok(calls)
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Vec<ToolDef> {
        vec![ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event".into()),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title"]
            }),
        }]
    }

    #[test]
    fn roundtrip_call() {
        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"Design review"}>>"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(result[0].name, "create_calendar_event");
        assert_eq!(result[0].arguments["title"], "Design review");
    }

    #[test]
    fn unknown_tool_errors() {
        let result = decode_calls(
            r#"<<call unknown {"title":"test"}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(CompactError::UnknownTool(_))));
    }

    #[test]
    fn missing_required_errors() {
        let result = decode_calls(
            r#"<<call create_calendar_event {}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(CompactError::MissingArgument(_))));
    }

    #[test]
    fn enum_validation() {
        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"x","visibility":"bad"}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(CompactError::InvalidArgument(_))));
    }

    #[test]
    fn marker_inside_string() {
        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"hello >> world"}>>"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(result[0].arguments["title"], "hello >> world");
    }

    #[test]
    fn stream_chunks() {
        let mut decoder = StreamDecoder::new();

        assert!(decoder
            .push("<<call create_calendar_event {\"title\":\"Des", &tools())
            .unwrap()
            .is_empty());

        let result = decoder
            .push("ign\"}>>", &tools())
            .unwrap();

        assert_eq!(result.len(), 1);
    }
}