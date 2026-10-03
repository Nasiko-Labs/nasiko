use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub parameters: Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    #[serde(rename = "type")]
    pub schema_type: String,

    #[serde(default)]
    pub properties: BTreeMap<String, SchemaProperty>,

    #[serde(default)]
    pub required: Vec<String>,

    #[serde(default)]
    pub additional_properties: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchemaProperty {
    #[serde(rename = "type")]
    pub property_type: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    pub description: Option<String>,

    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<Value>>,

    pub items: Option<Box<SchemaProperty>>,

    pub properties: Option<BTreeMap<String, SchemaProperty>>,

    pub required: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactTools {
    pub definitions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("invalid tool definition: {0}")]
    InvalidTool(String),

    #[error("unsupported schema feature: {0}")]
    UnsupportedSchema(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid tool call: {0}")]
    InvalidCall(String),

    #[error("invalid arguments for tool `{tool}`: {reason}")]
    InvalidArguments { tool: String, reason: String },

    #[error("malformed compact call")]
    MalformedCall,

    #[error("unterminated compact call")]
    UnterminatedCall,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Encode tool definitions into the compact P1 grammar.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = Vec::with_capacity(tools.len());

    for tool in tools {
        validate_tool(tool)?;

        let mut args = Vec::new();

        for (name, property) in &tool.parameters.properties {
            let required = tool.parameters.required.iter().any(|r| r == name);
            let ty = property_type(property)?;

            let prefix = if required {
                format!("{name}:{ty}")
            } else {
                format!("{name}?:{ty}")
            };

            let argument = match property.description.as_deref().map(str::trim) {
                Some(description) if !description.is_empty() => {
                    let encoded = serde_json::to_string(description)
                        .map_err(|error| Error::InvalidTool(error.to_string()))?;

                    format!("{prefix}({encoded})")
                }
                _ => prefix,
            };

            args.push(argument);
        }

        let mut line = format!("{}({})", tool.name, args.join(", "));

        if let Some(description) = &tool.description {
            let description = description.trim();

            if !description.is_empty() {
                line.push_str(" - ");
                line.push_str(description);
            }
        }

        lines.push(line);
    }

    Ok(CompactTools {
        definitions: lines.join("\n"),
    })
}

fn validate_tool(tool: &ToolDef) -> Result<()> {
    if tool.name.trim().is_empty() {
        return Err(Error::InvalidTool("tool name cannot be empty".to_string()));
    }

    if tool.parameters.schema_type != "object" {
        return Err(Error::UnsupportedSchema(format!(
            "root schema type `{}`",
            tool.parameters.schema_type
        )));
    }

    for required in &tool.parameters.required {
        if !tool.parameters.properties.contains_key(required) {
            return Err(Error::InvalidTool(format!(
                "required property `{required}` is not defined"
            )));
        }
    }

    if tool.parameters.additional_properties == Some(true) {
        return Err(Error::UnsupportedSchema(
            "additionalProperties=true".to_string(),
        ));
    }

    Ok(())
}

fn property_type(property: &SchemaProperty) -> Result<String> {
    if let Some(values) = &property.enum_values {
        if values.is_empty() {
            return Err(Error::InvalidTool(
                "enum must contain at least one value".to_string(),
            ));
        }

        let mut rendered = Vec::new();

        for value in values {
            let Some(value) = value.as_str() else {
                return Err(Error::UnsupportedSchema(
                    "only string enum values are supported".to_string(),
                ));
            };

            if value.contains('|')
                || value.contains(',')
                || value.contains(')')
                || value.contains('(')
            {
                return Err(Error::UnsupportedSchema(format!(
                    "enum value `{value}` contains grammar punctuation"
                )));
            }

            rendered.push(value.to_string());
        }

        let base = if property.property_type.as_deref() == Some("string")
            && property.format.as_deref() == Some("date-time")
        {
            "datetime"
        } else {
            property.property_type.as_deref().unwrap_or("str")
        };

        return Ok(format!("{base}|{}", rendered.join("|")));
    }

    if let Some(items) = &property.items {
        let item_type = property_type(items)?;
        return Ok(format!("[{item_type}]"));
    }

    if let Some(properties) = &property.properties {
        let mut nested = Vec::new();

        let required = property.required.as_deref().unwrap_or(&[]);

        for (name, child) in properties {
            let child_type = property_type(child)?;

            if required.iter().any(|r| r == name) {
                nested.push(format!("{name}:{child_type}"));
            } else {
                nested.push(format!("{name}?:{child_type}"));
            }
        }

        return Ok(format!("{{{}}}", nested.join(", ")));
    }

    if property.property_type.as_deref() == Some("string")
        && property.format.as_deref() == Some("date-time")
    {
        return Ok("datetime".to_string());
    }

    match property.property_type.as_deref() {
        Some("string") => Ok("str".to_string()),
        Some("integer") => Ok("int".to_string()),
        Some("number") => Ok("number".to_string()),
        Some("boolean") => Ok("bool".to_string()),
        Some("null") => Ok("null".to_string()),
        Some("object") => Err(Error::UnsupportedSchema(
            "object without properties".to_string(),
        )),
        Some("array") => Err(Error::UnsupportedSchema("array without items".to_string())),
        Some(other) => Err(Error::UnsupportedSchema(format!("property type `{other}`"))),
        None => Err(Error::InvalidTool("property has no type".to_string())),
    }
}

/// Decode one or more compact tool calls from model output.
///
/// Text outside `<<call ...>>` markers is ignored.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find("<<call ") {
        let start = cursor + relative_start;
        let body_start = start + "<<call ".len();

        let Some(relative_end) = find_call_end(&text[body_start..]) else {
            return Err(Error::UnterminatedCall);
        };

        let end = body_start + relative_end;
        let body = &text[body_start..end];

        let (name, arguments_text) = split_call_body(body)?;

        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

        let arguments: Value =
            serde_json::from_str(arguments_text).map_err(|_| Error::MalformedCall)?;

        validate_arguments(tool, &arguments)?;

        calls.push(ToolCall {
            name: name.to_string(),
            arguments,
        });

        cursor = end + ">>".len();
    }

    Ok(calls)
}

/// Incremental decoder for streaming model output.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
}

impl StreamDecoder {
    pub fn new(tools: Vec<ToolDef>) -> Self {
        Self {
            tools,
            buffer: String::new(),
        }
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);

        let mut calls = Vec::new();

        loop {
            let Some(start) = self.buffer.find("<<call ") else {
                const MARKER: &str = "<<call ";
                let keep = MARKER.len().saturating_sub(1);

                if self.buffer.len() > keep {
                    let drain_to = self.buffer.len() - keep;
                    self.buffer.drain(..drain_to);
                }

                break;
            };

            if start > 0 {
                self.buffer.drain(..start);
            }

            let body_start = "<<call ".len();

            let Some(relative_end) = find_call_end(&self.buffer[body_start..]) else {
                break;
            };

            let end = body_start + relative_end;
            let body = self.buffer[body_start..end].to_string();

            let (name, arguments_text) = split_call_body(&body)?;

            let tool = self
                .tools
                .iter()
                .find(|tool| tool.name == name)
                .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

            let arguments: Value =
                serde_json::from_str(arguments_text).map_err(|_| Error::MalformedCall)?;

            validate_arguments(tool, &arguments)?;

            calls.push(ToolCall {
                name: name.to_string(),
                arguments,
            });

            self.buffer.drain(..end + 2);
        }

        Ok(calls)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        if self.buffer.contains("<<call ") {
            return Err(Error::UnterminatedCall);
        }

        self.buffer.clear();

        Ok(Vec::new())
    }
}

/// Locate the closing `>>` while respecting JSON string escaping.
fn find_call_end(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();

    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];

        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }

            i += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            i += 1;
            continue;
        }

        if byte == b'>' && i + 1 < bytes.len() && bytes[i + 1] == b'>' {
            return Some(i);
        }

        i += 1;
    }

    None
}

fn split_call_body(body: &str) -> Result<(&str, &str)> {
    let body = body.trim();

    let Some(separator) = body.find(char::is_whitespace) else {
        return Err(Error::MalformedCall);
    };

    let name = body[..separator].trim();

    if name.is_empty() {
        return Err(Error::MalformedCall);
    }

    let arguments = body[separator..].trim();

    if arguments.is_empty() {
        return Err(Error::InvalidCall(format!(
            "tool `{name}` has no arguments"
        )));
    }

    if !arguments.starts_with('{') || !arguments.ends_with('}') {
        return Err(Error::MalformedCall);
    }

    Ok((name, arguments))
}

fn validate_arguments(tool: &ToolDef, arguments: &Value) -> Result<()> {
    let Some(object) = arguments.as_object() else {
        return Err(Error::InvalidArguments {
            tool: tool.name.clone(),
            reason: "arguments must be a JSON object".to_string(),
        });
    };

    for required in &tool.parameters.required {
        if !object.contains_key(required) {
            return Err(Error::InvalidArguments {
                tool: tool.name.clone(),
                reason: format!("missing required argument `{required}`"),
            });
        }
    }

    for (name, value) in object {
        let Some(property) = tool.parameters.properties.get(name) else {
            return Err(Error::InvalidArguments {
                tool: tool.name.clone(),
                reason: format!("unknown argument `{name}`"),
            });
        };

        validate_property(&tool.name, name, property, value)?;
    }

    Ok(())
}

fn validate_property(
    tool_name: &str,
    property_name: &str,
    property: &SchemaProperty,
    value: &Value,
) -> Result<()> {
    if let Some(enum_values) = &property.enum_values {
        if !enum_values.iter().any(|allowed| allowed == value) {
            return Err(Error::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!(
                    "invalid value for `{property_name}`; expected one of {:?}",
                    enum_values
                ),
            });
        }
    }

    if let Some(property_type) = &property.property_type {
        let valid = match property_type.as_str() {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),

            "array" => {
                let Some(items) = value.as_array() else {
                    return Err(Error::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("`{property_name}` must be an array"),
                    });
                };

                if let Some(item_schema) = &property.items {
                    for item in items {
                        validate_property(tool_name, property_name, item_schema, item)?;
                    }
                }

                true
            }

            "object" => {
                let Some(object) = value.as_object() else {
                    return Err(Error::InvalidArguments {
                        tool: tool_name.to_string(),
                        reason: format!("`{property_name}` must be an object"),
                    });
                };

                if let Some(properties) = &property.properties {
                    let required = property.required.as_deref().unwrap_or(&[]);

                    for required_name in required {
                        if !object.contains_key(required_name) {
                            return Err(Error::InvalidArguments {
                                tool: tool_name.to_string(),
                                reason: format!(
                                    "missing required nested argument `{property_name}.{required_name}`"
                                ),
                            });
                        }
                    }

                    for (nested_name, nested_value) in object {
                        let Some(nested_schema) = properties.get(nested_name) else {
                            return Err(Error::InvalidArguments {
                                tool: tool_name.to_string(),
                                reason: format!(
                                    "unknown nested argument `{property_name}.{nested_name}`"
                                ),
                            });
                        };

                        validate_property(
                            tool_name,
                            &format!("{property_name}.{nested_name}"),
                            nested_schema,
                            nested_value,
                        )?;
                    }
                }

                true
            }

            _ => false,
        };

        if !valid {
            return Err(Error::InvalidArguments {
                tool: tool_name.to_string(),
                reason: format!("`{property_name}` has the wrong type; expected `{property_type}`"),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> ToolDef {
        let mut properties = BTreeMap::new();

        properties.insert(
            "title".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        properties.insert(
            "duration_min".into(),
            SchemaProperty {
                property_type: Some("integer".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        properties.insert(
            "visibility".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: None,
                description: None,
                enum_values: Some(vec![json!("public"), json!("private")]),
                items: None,
                properties: None,
                required: None,
            },
        );

        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event.".into()),
            parameters: Schema {
                schema_type: "object".into(),
                properties,
                required: vec!["title".into()],
                additional_properties: Some(false),
            },
        }
    }

    fn echo_tool() -> ToolDef {
        let mut properties = BTreeMap::new();

        properties.insert(
            "message".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        ToolDef {
            name: "echo".into(),
            description: Some("Echo a message.".into()),
            parameters: Schema {
                schema_type: "object".into(),
                properties,
                required: vec!["message".into()],
                additional_properties: Some(false),
            },
        }
    }

    #[test]
    fn preserves_property_description() {
        let mut t = tool();

        t.parameters
            .properties
            .get_mut("title")
            .unwrap()
            .description = Some("Calendar event title".into());

        let compact = encode_tools(&[t]).unwrap();

        assert!(
            compact
                .definitions
                .contains(r#"title:str("Calendar event title")"#)
        );
    }

    #[test]
    fn preserves_property_description_with_json_escaping() {
        let mut t = tool();

        t.parameters
            .properties
            .get_mut("title")
            .unwrap()
            .description = Some(r#"Title containing "quotes" and commas, safely."#.into());

        let compact = encode_tools(&[t]).unwrap();

        let expected = "title:str(\"Title containing \\\"quotes\\\" and commas, safely.\")";
        assert!(compact.definitions.contains(expected));
    }

    #[test]
    fn encodes_required_optional_and_enum() {
        let compact = encode_tools(&[tool()]).unwrap();

        assert_eq!(
            compact.definitions,
            "create_calendar_event(duration_min?:int, title:str, visibility?:string|public|private) - Create an event."
        );
    }

    #[test]
    fn encodes_datetime() {
        let mut t = tool();

        t.parameters.properties.insert(
            "start".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: Some("date-time".into()),
                description: Some("Event start time".into()),
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        t.parameters.required.push("start".into());

        let compact = encode_tools(&[t]).unwrap();

        assert!(compact.definitions.contains("start:datetime"));
    }

    #[test]
    fn preserves_supported_property_description_with_constraint_text() {
        let mut t = tool();
        t.parameters.properties.insert(
            "name".to_string(),
            SchemaProperty {
                property_type: Some("string".to_string()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        // The compact representation cannot preserve constraints such as
        // pattern/minLength/maxLength, so the caller must bypass compaction.
        let mut property = t.parameters.properties["name"].clone();
        property.description = Some("pattern=^[A-Z]+$".to_string());

        // Description itself remains valid metadata; this test verifies the
        // library continues to encode the supported schema subset.
        t.parameters.properties.insert("name".to_string(), property);

        assert!(encode_tools(&[t]).is_ok());
    }

    #[test]
    fn rejects_additional_properties_true() {
        let mut t = tool();
        t.parameters.additional_properties = Some(true);

        assert!(matches!(
            encode_tools(&[t]),
            Err(Error::UnsupportedSchema(reason))
                if reason == "additionalProperties=true"
        ));
    }

    #[test]
    fn rejects_non_object_root() {
        let mut t = tool();
        t.parameters.schema_type = "array".into();

        assert!(matches!(
            encode_tools(&[t]),
            Err(Error::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn rejects_unknown_required_property() {
        let mut t = tool();
        t.parameters.required.push("missing".into());

        assert!(matches!(encode_tools(&[t]), Err(Error::InvalidTool(_))));
    }

    #[test]
    fn encodes_string_array() {
        let mut t = tool();

        t.parameters.properties.insert(
            "attendees".into(),
            SchemaProperty {
                property_type: Some("array".into()),
                format: None,
                description: None,
                enum_values: None,
                items: Some(Box::new(SchemaProperty {
                    property_type: Some("string".into()),
                    format: None,
                    description: None,
                    enum_values: None,
                    items: None,
                    properties: None,
                    required: None,
                })),
                properties: None,
                required: None,
            },
        );

        let compact = encode_tools(&[t]).unwrap();

        assert!(compact.definitions.contains("attendees?:[str]"));
    }

    #[test]
    fn decodes_single_call() {
        let tools = vec![tool()];

        let text = r#"Some text <<call create_calendar_event {"title":"Design review","duration_min":60}>> more text"#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], "Design review");
        assert_eq!(calls[0].arguments["duration_min"], 60);
    }

    #[test]
    fn decodes_multiple_calls_and_ignores_text() {
        let tools = vec![tool()];

        let text = r#"
            I will create these events.

            <<call create_calendar_event {"title":"First"}>>

            Done.

            <<call create_calendar_event {"title":"Second"}>>
        "#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["title"], "First");
        assert_eq!(calls[1].arguments["title"], "Second");
    }

    #[test]
    fn rejects_unknown_tool() {
        let tools = vec![tool()];

        let result = decode_calls(r#"<<call unknown_tool {"value":"test"}>>"#, &tools);

        assert!(matches!(
            result,
            Err(Error::UnknownTool(name)) if name == "unknown_tool"
        ));
    }

    #[test]
    fn rejects_missing_required_argument() {
        let tools = vec![tool()];

        let result = decode_calls(r#"<<call create_calendar_event {}>>"#, &tools);

        assert!(matches!(
            result,
            Err(Error::InvalidArguments { tool, .. })
                if tool == "create_calendar_event"
        ));
    }

    #[test]
    fn rejects_invalid_enum_value() {
        let tools = vec![tool()];

        let result = decode_calls(
            r#"<<call create_calendar_event {"title":"Test","visibility":"secret"}>>"#,
            &tools,
        );

        assert!(matches!(
            result,
            Err(Error::InvalidArguments { tool, .. })
                if tool == "create_calendar_event"
        ));
    }

    #[test]
    fn allows_gt_gt_inside_json_string() {
        let tools = vec![echo_tool()];

        let calls = decode_calls(
            r#"<<call echo {"message":"value >> inside string"}>>"#,
            &tools,
        )
        .unwrap();

        assert_eq!(calls[0].arguments["message"], "value >> inside string");
    }

    #[test]
    fn rejects_unterminated_call() {
        let tools = vec![echo_tool()];

        let result = decode_calls(r#"<<call echo {"message":"hello"}"#, &tools);

        assert!(matches!(result, Err(Error::UnterminatedCall)));
    }

    #[test]
    fn stream_decoder_handles_split_marker() {
        let mut decoder = StreamDecoder::new(vec![echo_tool()]);

        assert!(decoder.push("Hello <<cal").unwrap().is_empty());
        assert!(decoder.push("l echo ").unwrap().is_empty());

        let calls = decoder.push(r#"{"message":"hello"}>>"#).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(calls[0].arguments["message"], "hello");
    }

    #[test]
    fn stream_decoder_handles_split_end_marker() {
        let mut decoder = StreamDecoder::new(vec![echo_tool()]);

        assert!(
            decoder
                .push(r#"<<call echo {"message":"hello"}>"#)
                .unwrap()
                .is_empty()
        );

        let calls = decoder.push(">").unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
    }

    #[test]
    fn stream_decoder_handles_multiple_calls() {
        let mut decoder = StreamDecoder::new(vec![echo_tool()]);

        let calls = decoder
            .push(
                r#"text <<call echo {"message":"one"}>> middle <<call echo {"message":"two"}>> end"#,
            )
            .unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments["message"], "one");
        assert_eq!(calls[1].arguments["message"], "two");
    }
    #[test]
    fn decodes_call_with_text_around_it() {
        let tools = vec![echo_tool()];

        let text = r#"Before <<call echo {"message":"hello"}>> after"#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(calls[0].arguments["message"], "hello");
    }

    #[test]
    fn rejects_unknown_argument() {
        let tools = vec![echo_tool()];

        let text = r#"<<call echo {"message":"hello","extra":"bad"}>>"#;

        assert!(matches!(
            decode_calls(text, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn rejects_invalid_enum() {
        let tools = vec![tool()];

        let text = r#"<<call create_calendar_event {"title":"Test","visibility":"secret"}>>"#;

        assert!(matches!(
            decode_calls(text, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn rejects_wrong_argument_type() {
        let tools = vec![tool()];

        let text = r#"<<call create_calendar_event {"title":123}>>"#;

        assert!(matches!(
            decode_calls(text, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn handles_arrow_marker_inside_string() {
        let tools = vec![echo_tool()];

        let text = r#"<<call echo {"message":"hello >> world"}>>"#;

        let calls = decode_calls(text, &tools).unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(calls[0].arguments["message"], "hello >> world");
    }

    #[test]
    fn encodes_nested_object() {
        let mut t = echo_tool();

        let mut nested_properties = BTreeMap::new();

        nested_properties.insert(
            "city".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        nested_properties.insert(
            "zip".into(),
            SchemaProperty {
                property_type: Some("integer".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        t.parameters.properties.insert(
            "address".into(),
            SchemaProperty {
                property_type: Some("object".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: Some(nested_properties),
                required: Some(vec!["city".into()]),
            },
        );

        let compact = encode_tools(&[t]).unwrap();

        assert!(
            compact
                .definitions
                .contains("address?:{city:str, zip?:int}")
        );
    }

    #[test]
    fn validates_nested_object() {
        let mut t = echo_tool();

        let mut nested_properties = BTreeMap::new();

        nested_properties.insert(
            "city".into(),
            SchemaProperty {
                property_type: Some("string".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: None,
                required: None,
            },
        );

        t.parameters.properties.insert(
            "address".into(),
            SchemaProperty {
                property_type: Some("object".into()),
                format: None,
                description: None,
                enum_values: None,
                items: None,
                properties: Some(nested_properties),
                required: Some(vec!["city".into()]),
            },
        );

        t.parameters.required.push("address".into());

        let tools = vec![t];

        let valid = r#"<<call echo {"message":"hi","address":{"city":"Nellore"}}>>"#;

        assert!(decode_calls(valid, &tools).is_ok());

        let invalid = r#"<<call echo {"message":"hi","address":{}}>>"#;

        assert!(matches!(
            decode_calls(invalid, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn validates_array_items() {
        let mut t = echo_tool();

        t.parameters.properties.insert(
            "tags".into(),
            SchemaProperty {
                property_type: Some("array".into()),
                format: None,
                description: None,
                enum_values: None,
                items: Some(Box::new(SchemaProperty {
                    property_type: Some("string".into()),
                    format: None,
                    description: None,
                    enum_values: None,
                    items: None,
                    properties: None,
                    required: None,
                })),
                properties: None,
                required: None,
            },
        );

        let tools = vec![t];

        let valid = r#"<<call echo {"message":"hi","tags":["a","b"]}>>"#;

        assert!(decode_calls(valid, &tools).is_ok());

        let invalid = r#"<<call echo {"message":"hi","tags":["a",123]}>>"#;

        assert!(matches!(
            decode_calls(invalid, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn stream_decoder_handles_split_closing_marker() {
        let tools = vec![echo_tool()];
        let mut decoder = StreamDecoder::new(tools);

        assert!(
            decoder
                .push(r#"<<call echo {"message":"hello"}>"#)
                .unwrap()
                .is_empty()
        );

        let calls = decoder.push(">").unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["message"], "hello");
    }

    #[test]
    fn stream_decoder_finish_rejects_incomplete_call() {
        let tools = vec![echo_tool()];
        let mut decoder = StreamDecoder::new(tools);

        decoder.push(r#"<<call echo {"message":"hello"}"#).unwrap();

        assert!(matches!(decoder.finish(), Err(Error::UnterminatedCall)));
    }
}
