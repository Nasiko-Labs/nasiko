use crate::error::Error;
use crate::model::{CompactTools, ToolCall};
use serde_json::Value;

const CALL_START: &str = "<<call ";
const CALL_END: &str = ">>";

pub fn decode_calls(text: &str, tools: &CompactTools) -> Result<Vec<ToolCall>, Error> {
    let mut calls = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = text[cursor..].find(CALL_START) {
        let start = cursor + relative_start;
        let content_start = start + CALL_START.len();

        let Some(relative_end) = find_call_end(&text[content_start..]) else {
            return Err(Error::InvalidFormat(
                "unterminated tool call".to_string(),
            ));
        };

        let end = content_start + relative_end;
        let body = text[content_start..end].trim();

        let split_at = body.find(char::is_whitespace).ok_or_else(|| {
            Error::InvalidFormat(
                "tool call must contain a tool name and JSON arguments".to_string(),
            )
        })?;

        let tool_name = body[..split_at].trim();
        let arguments_text = body[split_at..].trim();

        if tool_name.is_empty() {
            return Err(Error::InvalidFormat(
                "tool name cannot be empty".to_string(),
            ));
        }

        if !tools.lookup.contains_key(tool_name) {
            return Err(Error::UnknownTool(tool_name.to_string()));
        }

        let arguments: Value = serde_json::from_str(arguments_text).map_err(|error| {
            Error::InvalidArguments(format!(
                "tool `{tool_name}` contains invalid JSON: {error}"
            ))
        })?;

        validate_arguments(tool_name, &arguments, tools)?;

        calls.push(ToolCall {
            name: tool_name.to_string(),
            arguments: arguments.to_string(),
        });

        cursor = end + CALL_END.len();
    }

    Ok(calls)
}

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
        } else if byte == b'>' && bytes[i + 1] == b'>' {
            return Some(i);
        }

        i += 1;
    }

    None
}

fn validate_arguments(
    tool_name: &str,
    arguments: &Value,
    tools: &CompactTools,
) -> Result<(), Error> {
    let index = tools
        .lookup
        .get(tool_name)
        .ok_or_else(|| Error::UnknownTool(tool_name.to_string()))?;

    let tool = &tools.tools[*index];

    let Some(schema) = &tool.parameters else {
        return Ok(());
    };

    let Some(schema_object) = schema.as_object() else {
        return Err(Error::UnsupportedSchema(format!(
            "tool `{tool_name}` parameters must be an object"
        )));
    };

    let Some(argument_object) = arguments.as_object() else {
        return Err(Error::InvalidArguments(format!(
            "tool `{tool_name}` arguments must be a JSON object"
        )));
    };

    if let Some(required) = schema_object.get("required").and_then(Value::as_array) {
        for required_name in required {
            let Some(name) = required_name.as_str() else {
                return Err(Error::UnsupportedSchema(format!(
                    "tool `{tool_name}` contains a non-string required field"
                )));
            };

            if !argument_object.contains_key(name) {
                return Err(Error::MissingRequiredArgument(name.to_string()));
            }
        }
    }

    if let Some(properties) = schema_object.get("properties").and_then(Value::as_object) {
        for (name, value) in argument_object {
            let Some(property_schema) = properties.get(name) else {
                continue;
            };

            validate_property(tool_name, name, value, property_schema)?;
        }
    }

    Ok(())
}

fn validate_property(
    tool_name: &str,
    argument_name: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), Error> {
    let Some(schema_object) = schema.as_object() else {
        return Err(Error::UnsupportedSchema(format!(
            "tool `{tool_name}` argument `{argument_name}` has an invalid schema"
        )));
    };

    if let Some(expected_type) = schema_object.get("type").and_then(Value::as_str) {
        let valid = match expected_type {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            "null" => value.is_null(),
            other => {
                return Err(Error::UnsupportedSchema(format!(
                    "tool `{tool_name}` argument `{argument_name}` uses unsupported type `{other}`"
                )));
            }
        };

        if !valid {
            return Err(Error::InvalidArgumentType(format!(
                "tool `{tool_name}` argument `{argument_name}` expected `{expected_type}`"
            )));
        }
    }

    if let Some(enum_values) = schema_object.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|allowed| allowed == value) {
            return Err(Error::InvalidEnumValue(format!(
                "tool `{tool_name}` argument `{argument_name}` has a value outside the allowed enum"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode_tools;
    use crate::model::ToolDef;
    use serde_json::json;

    fn tools() -> CompactTools {
        encode_tools(&[ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get weather".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    },
                    "units": {
                        "type": "string",
                        "enum": ["celsius", "fahrenheit"]
                    }
                },
                "required": ["city"]
            })),
        }])
        .unwrap()
    }

    #[test]
    fn decodes_valid_call() {
        let result = decode_calls(
            r#"before <<call get_weather {"city":"Hyderabad","units":"celsius"}>> after"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_weather");
    }

    #[test]
    fn decodes_multiple_calls() {
        let result = decode_calls(
            r#"<<call get_weather {"city":"Hyderabad"}>> <<call get_weather {"city":"Delhi"}>>"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn allows_end_marker_inside_json_string() {
        let result = decode_calls(
            r#"<<call get_weather {"city":"Hyderabad >> India"}>>"#,
            &tools(),
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "get_weather");

        let args: serde_json::Value =
            serde_json::from_str(&result[0].arguments).unwrap();

        assert_eq!(args["city"], "Hyderabad >> India");
    }

    #[test]
    fn rejects_unknown_tool() {
        let result = decode_calls(
            r#"<<call unknown {"city":"Hyderabad"}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(Error::UnknownTool(_))));
    }

    #[test]
    fn rejects_missing_required_argument() {
        let result = decode_calls(
            r#"<<call get_weather {"units":"celsius"}>>"#,
            &tools(),
        );

        assert!(matches!(
            result,
            Err(Error::MissingRequiredArgument(_))
        ));
    }

    #[test]
    fn rejects_invalid_type() {
        let result = decode_calls(
            r#"<<call get_weather {"city":123}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(Error::InvalidArgumentType(_))));
    }

    #[test]
    fn rejects_invalid_enum() {
        let result = decode_calls(
            r#"<<call get_weather {"city":"Hyderabad","units":"kelvin"}>>"#,
            &tools(),
        );

        assert!(matches!(result, Err(Error::InvalidEnumValue(_))));
    }

    #[test]
    fn returns_empty_for_plain_text() {
        let result = decode_calls("Hello, I can help you with that.", &tools()).unwrap();

        assert!(result.is_empty());
    }
}
