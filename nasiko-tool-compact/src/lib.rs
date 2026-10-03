//! Compact tool-schema grammar.
//!
//! Each tool is rendered as name(field:type, optional?:type, enum?:a|b).
//! A model emits calls as <<call name {json args}>>.
//! JSON arguments are parsed structurally, so escaped quotes and >> inside
//! JSON strings are preserved. Multiple calls and surrounding text are allowed.
//! Plain text produces zero calls.
//!
//! Required fields are unmarked; optional fields use ?. Supported types are
//! str, datetime, int, number, bool, nested objects, arrays, and string enums.
//! Unsupported JSON Schema features fail closed instead of being silently changed.
//!
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    rendered: String,
}

impl CompactTools {
    pub fn as_str(&self) -> &str {
        &self.rendered
    }
}

impl std::fmt::Display for CompactTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("tool name is empty")]
    EmptyToolName,

    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),

    #[error("invalid tool name: {0}")]
    InvalidToolName(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("malformed call marker")]
    MalformedCall,

    #[error("invalid JSON arguments: {0}")]
    InvalidJson(String),

    #[error("invalid arguments for {tool}: {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("unsupported schema feature at {path}: {feature}")]
    UnsupportedSchema { path: String, feature: String },
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    validate_tools(tools)?;

    let mut output = String::from("Available tools:\n");

    for tool in tools {
        output.push_str(&tool.name);
        output.push('(');

        if let Some(schema) = &tool.parameters {
            let fields = render_object_schema(schema, &tool.name)?;
            output.push_str(&fields);
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

    output.push_str("To call a tool, emit: <<call name {json args}>>");

    Ok(CompactTools { rendered: output })
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    validate_tools(tools)?;

    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    let calls = decoder.finish()?;

    Ok(calls)
}

pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    buffer: String,
    calls: Vec<ToolCall>,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef]) -> Result<Self, Error> {
        validate_tools(tools)?;

        Ok(Self {
            tools,
            buffer: String::new(),
            calls: Vec::new(),
        })
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, Error> {
        self.buffer.push_str(chunk);

        let mut produced = Vec::new();

        loop {
            let Some(start) = self.buffer.find("<<call ") else {
                if self.buffer.len() > 7 {
                    let keep_from = self.buffer.len().saturating_sub(7);
                    self.buffer.drain(..keep_from);
                }
                break;
            };

            if start > 0 {
                self.buffer.drain(..start);
            }

            let Some(open_brace) = self.buffer.find('{') else {
                break;
            };

            let Some(close_brace) = find_json_object_end(&self.buffer, open_brace) else {
                break;
            };

            let marker_end = close_brace + 3;

            if self.buffer.len() < marker_end {
                break;
            }

            if &self.buffer[close_brace..marker_end] != "}>>" {
                return Err(Error::MalformedCall);
            }

            let header = &self.buffer["<<call ".len()..open_brace];
            let name = header.trim();

            if name.is_empty() || !is_valid_tool_name(name) {
                return Err(Error::MalformedCall);
            }

            let arguments_text = &self.buffer[open_brace..=close_brace];
            let arguments: Value = serde_json::from_str(arguments_text)
                .map_err(|e| Error::InvalidJson(e.to_string()))?;

            let tool = self
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

            validate_arguments(tool, &arguments)?;

            let call = ToolCall {
                name: name.to_string(),
                arguments,
            };

            self.calls.push(call.clone());
            produced.push(call);

            self.buffer.drain(..marker_end);
        }

        Ok(produced)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>, Error> {
        if self.buffer.contains("<<call") {
            return Err(Error::MalformedCall);
        }

        Ok(std::mem::take(&mut self.calls))
    }
}

fn validate_tools(tools: &[ToolDef]) -> Result<(), Error> {
    let mut names = std::collections::HashSet::new();

    for tool in tools {
        if tool.name.trim().is_empty() {
            return Err(Error::EmptyToolName);
        }

        if !is_valid_tool_name(&tool.name) {
            return Err(Error::InvalidToolName(tool.name.clone()));
        }

        if !names.insert(tool.name.clone()) {
            return Err(Error::DuplicateTool(tool.name.clone()));
        }

        if let Some(schema) = &tool.parameters {
            validate_schema(schema, "$")?;
        }
    }

    Ok(())
}

fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn render_object_schema(schema: &Value, path: &str) -> Result<String, Error> {
    let object = schema.as_object().ok_or_else(|| Error::UnsupportedSchema {
        path: path.to_string(),
        feature: "root schema must be an object".into(),
    })?;

    if object.get("type").and_then(Value::as_str) != Some("object") {
        return Err(Error::UnsupportedSchema {
            path: path.to_string(),
            feature: "root schema must have type object".into(),
        });
    }

    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::UnsupportedSchema {
            path: format!("{path}.properties"),
            feature: "properties is required".into(),
        })?;

    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let mut fields = Vec::new();

    for (name, property) in properties {
        let optional = !required.contains(name.as_str());
        let rendered_type = render_schema_type(property, &format!("{path}.properties.{name}"))?;

        let field_name = if optional {
            format!("{name}?")
        } else {
            name.clone()
        };

        fields.push(format!("{field_name}:{rendered_type}"));
    }

    Ok(fields.join(", "))
}

fn render_schema_type(schema: &Value, path: &str) -> Result<String, Error> {
    let object = schema.as_object().ok_or_else(|| Error::UnsupportedSchema {
        path: path.to_string(),
        feature: "schema must be an object".into(),
    })?;

    if let Some(enum_values) = object.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or_else(|| Error::UnsupportedSchema {
                path: path.to_string(),
                feature: "enum must be an array".into(),
            })?;

        if values.is_empty() {
            return Err(Error::UnsupportedSchema {
                path: path.to_string(),
                feature: "enum must not be empty".into(),
            });
        }

        let mut rendered = Vec::new();

        for value in values {
            let value = value.as_str().ok_or_else(|| Error::UnsupportedSchema {
                path: path.to_string(),
                feature: "only string enums are supported".into(),
            })?;

            rendered.push(value.to_string());
        }

        return Ok(rendered.join("|"));
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") => {
            if let Some(format) = object.get("format").and_then(Value::as_str) {
                if format == "date-time" {
                    return Ok("datetime".into());
                }
            }
            Ok("str".into())
        }

        Some("integer") => Ok("int".into()),

        Some("number") => Ok("number".into()),

        Some("boolean") => Ok("bool".into()),

        Some("object") => {
            let properties = object.get("properties");

            if properties.is_none() {
                return Err(Error::UnsupportedSchema {
                    path: path.to_string(),
                    feature: "object without properties".into(),
                });
            }

            Ok(format!("{{{}}}", render_object_schema(schema, path)?))
        }

        Some("array") => {
            let items = object
                .get("items")
                .ok_or_else(|| Error::UnsupportedSchema {
                    path: path.to_string(),
                    feature: "array items are required".into(),
                })?;

            Ok(format!(
                "[{}]",
                render_schema_type(items, &format!("{path}.items"))?
            ))
        }

        Some(other) => Err(Error::UnsupportedSchema {
            path: path.to_string(),
            feature: format!("unsupported type '{other}'"),
        }),

        None => Err(Error::UnsupportedSchema {
            path: path.to_string(),
            feature: "missing type".into(),
        }),
    }
}

fn validate_schema(schema: &Value, path: &str) -> Result<(), Error> {
    let object = schema.as_object().ok_or_else(|| Error::UnsupportedSchema {
        path: path.to_string(),
        feature: "schema must be an object".into(),
    })?;

    if let Some(enum_values) = object.get("enum") {
        let values = enum_values
            .as_array()
            .ok_or_else(|| Error::UnsupportedSchema {
                path: path.to_string(),
                feature: "enum must be an array".into(),
            })?;

        if values.is_empty() || values.iter().any(|value| !value.is_string()) {
            return Err(Error::UnsupportedSchema {
                path: path.to_string(),
                feature: "only non-empty string enums are supported".into(),
            });
        }

        return Ok(());
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") | Some("integer") | Some("number") | Some("boolean") => Ok(()),

        Some("object") => {
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| Error::UnsupportedSchema {
                    path: format!("{path}.properties"),
                    feature: "properties is required".into(),
                })?;

            if let Some(required) = object.get("required") {
                let required = required
                    .as_array()
                    .ok_or_else(|| Error::UnsupportedSchema {
                        path: format!("{path}.required"),
                        feature: "required must be an array".into(),
                    })?;

                for name in required {
                    let name = name.as_str().ok_or_else(|| Error::UnsupportedSchema {
                        path: format!("{path}.required"),
                        feature: "required entries must be strings".into(),
                    })?;

                    if !properties.contains_key(name) {
                        return Err(Error::UnsupportedSchema {
                            path: format!("{path}.required"),
                            feature: format!("unknown required property '{name}'"),
                        });
                    }
                }
            }

            for (name, property) in properties {
                validate_schema(property, &format!("{path}.properties.{name}"))?;
            }

            Ok(())
        }

        Some("array") => {
            let items = object
                .get("items")
                .ok_or_else(|| Error::UnsupportedSchema {
                    path: format!("{path}.items"),
                    feature: "array items are required".into(),
                })?;

            validate_schema(items, &format!("{path}.items"))
        }

        Some(other) => Err(Error::UnsupportedSchema {
            path: path.to_string(),
            feature: format!("unsupported type '{other}'"),
        }),

        None => Err(Error::UnsupportedSchema {
            path: path.to_string(),
            feature: "missing type".into(),
        }),
    }
}

fn validate_arguments(tool: &ToolDef, value: &Value) -> Result<(), Error> {
    let schema = match &tool.parameters {
        Some(schema) => schema,
        None => return Ok(()),
    };

    validate_value_against_schema(value, schema, &format!("arguments for {}", tool.name)).map_err(
        |reason| Error::InvalidArguments {
            tool: tool.name.clone(),
            reason,
        },
    )
}

fn validate_value_against_schema(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: invalid schema"))?;

    if let Some(enum_values) = object.get("enum") {
        let actual = value
            .as_str()
            .ok_or_else(|| format!("{path}: enum value must be a string"))?;

        let valid = enum_values
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|candidate| candidate == actual)
            })
            .unwrap_or(false);

        if !valid {
            return Err(format!("{path}: invalid enum value '{actual}'"));
        }

        return Ok(());
    }

    match object.get("type").and_then(Value::as_str) {
        Some("string") => {
            if value.is_string() {
                Ok(())
            } else {
                Err(format!("{path}: expected string"))
            }
        }

        Some("integer") => {
            if value.as_i64().is_some() || value.as_u64().is_some() {
                Ok(())
            } else {
                Err(format!("{path}: expected integer"))
            }
        }

        Some("number") => {
            if value.is_number() {
                Ok(())
            } else {
                Err(format!("{path}: expected number"))
            }
        }

        Some("boolean") => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(format!("{path}: expected boolean"))
            }
        }

        Some("object") => {
            let actual = value
                .as_object()
                .ok_or_else(|| format!("{path}: expected object"))?;

            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{path}: invalid object schema"))?;

            let required = object
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            for required_name in required {
                let name = required_name
                    .as_str()
                    .ok_or_else(|| format!("{path}: invalid required field"))?;

                if !actual.contains_key(name) {
                    return Err(format!("{path}: missing required field '{name}'"));
                }
            }

            for (name, property_schema) in properties {
                if let Some(property_value) = actual.get(name) {
                    validate_value_against_schema(
                        property_value,
                        property_schema,
                        &format!("{path}.{name}"),
                    )?;
                }
            }

            Ok(())
        }

        Some("array") => {
            let actual = value
                .as_array()
                .ok_or_else(|| format!("{path}: expected array"))?;

            let item_schema = object
                .get("items")
                .ok_or_else(|| format!("{path}: invalid array schema"))?;

            for (index, item) in actual.iter().enumerate() {
                validate_value_against_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }

            Ok(())
        }

        Some(other) => Err(format!("{path}: unsupported type '{other}'")),

        None => Err(format!("{path}: schema type missing")),
    }
}

fn find_json_object_end(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();

    if bytes.get(start) != Some(&b'{') {
        return None;
    }

    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for index in start..bytes.len() {
        let byte = bytes[index];

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
                depth = depth.checked_sub(1)?;

                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
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
                })),
            },
            ToolDef {
                name: "send_email".into(),
                description: Some("Send an email.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {
                            "type": "array",
                            "items": {"type": "string"}
                        },
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        ]
    }

    #[test]
    fn encodes_tools() {
        let encoded = encode_tools(&tools()).unwrap().to_string();

        assert!(encoded.contains("create_calendar_event"));
        assert!(encoded.contains("title:str"));
        assert!(encoded.contains("start:datetime"));
        assert!(encoded.contains("duration_min?:int"));
        assert!(encoded.contains("attendees?:[str]"));
        assert!(encoded.contains("visibility?:public|private"));
        assert!(encoded.contains("<<call name {json args}>>"));
    }

    #[test]
    fn decodes_multiple_calls() {
        let input = r#"
            Some text.
            <<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>
            More text.
            <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>
        "#;

        let calls = decode_calls(input, &tools()).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn handles_marker_inside_string() {
        let input = r#"<<call create_calendar_event {"title":"Design >> review","start":"2026-10-05T15:00:00+05:30"}>>"#;

        let calls = decode_calls(input, &tools()).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], "Design >> review");
    }

    #[test]
    fn handles_split_marker() {
        let test_tools = tools();
        let mut decoder = StreamDecoder::new(&test_tools).unwrap();

        assert!(decoder.push("<<ca").unwrap().is_empty());

        assert!(decoder
            .push(r#"ll create_calendar_event {"title":"Ret"#)
            .unwrap()
            .is_empty());

        let calls = decoder
            .push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>>"#)
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
    }

    #[test]
    fn plain_text_produces_no_calls() {
        assert!(decode_calls("What's the weather?", &tools())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_unknown_tool() {
        let error = decode_calls(r#"<<call nope {}>>"#, &tools()).unwrap_err();

        assert_eq!(error, Error::UnknownTool("nope".into()));
    }

    #[test]
    fn rejects_missing_required() {
        let error =
            decode_calls(r#"<<call create_calendar_event {"title":"x"}>>"#, &tools()).unwrap_err();

        assert!(matches!(error, Error::InvalidArguments { .. }));
    }

    #[test]
    fn rejects_invalid_enum() {
        let error = decode_calls(
            r#"<<call create_calendar_event {"title":"x","start":"t","visibility":"secret"}>>"#,
            &tools(),
        )
        .unwrap_err();

        assert!(matches!(error, Error::InvalidArguments { .. }));
    }

    #[test]
    fn validates_nested_objects_and_arrays() {
        let nested = vec![ToolDef {
            name: "f".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "meta": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "integer"}
                        },
                        "required": ["id"]
                    },
                    "items": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "x": {"type": "boolean"}
                            },
                            "required": ["x"]
                        }
                    }
                },
                "required": ["meta", "items"]
            })),
        }];

        let result = decode_calls(
            r#"<<call f {"meta":{"id":1},"items":[{"x":true}]}>>"#,
            &nested,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn rejects_wrong_nested_value() {
        let nested = vec![ToolDef {
            name: "f".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": {"type": "integer"}
                    }
                },
                "required": ["items"]
            })),
        }];

        let result = decode_calls(r#"<<call f {"items":[1,"bad",3]}>>"#, &nested);

        assert!(result.is_err());
    }
}
