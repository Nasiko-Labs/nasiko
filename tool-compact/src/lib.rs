use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A compact representation of a tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactTool {
    pub name: String,
    pub parameters: Vec<CompactParameter>,
}

/// A parameter in a compact tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactParameter {
    pub name: String,
    pub kind: CompactType,
    pub required: bool,
}

/// Supported parameter types for P1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactType {
    String,
    Integer,
    Boolean,
    Array,
    Object,
    Enum(Vec<String>),
}

/// A decoded tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactToolCall {
    pub name: String,
    pub arguments: BTreeMap<String, serde_json::Value>,
}
/// Encodes a normal tool definition into the compact representation.
pub fn encode_tool(
    name: impl Into<String>,
    parameters: Vec<CompactParameter>,
) -> CompactTool {
    CompactTool {
        name: name.into(),
        parameters,
    }
}
/// Encodes a JSON Schema parameter object into the compact representation.
pub fn encode_tool_schema(
    name: impl Into<String>,
    schema: &serde_json::Value,
) -> Result<CompactTool, String> {
    let properties = schema
        .get("properties")
        .and_then(|value| value.as_object())
        .ok_or_else(|| "tool schema must contain properties".to_string())?;

    let required = schema
        .get("required")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut parameters = Vec::new();

    for (parameter_name, parameter_schema) in properties {
        let kind = if let Some(enum_values) =
            parameter_schema.get("enum").and_then(|value| value.as_array())
        {
            let values = enum_values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>();

            CompactType::Enum(values)
        } else {
            match parameter_schema
                .get("type")
                .and_then(|value| value.as_str())
            {
                Some("string") => CompactType::String,
                Some("integer") => CompactType::Integer,
                Some("boolean") => CompactType::Boolean,
                Some("array") => CompactType::Array,
                Some("object") => CompactType::Object,
                Some(other) => {
                    return Err(format!(
                        "unsupported parameter type: {other}"
                    ));
                }
                None => {
                    return Err(format!(
                        "missing type for parameter: {parameter_name}"
                    ));
                }
            }
        };

        parameters.push(CompactParameter {
            name: parameter_name.clone(),
            kind,
            required: required.contains(parameter_name.as_str()),
        });
    }

    Ok(CompactTool {
        name: name.into(),
        parameters,
    })
}
/// Decodes a compact tool call.
pub fn decode_tool_call(input: &str) -> Result<CompactToolCall, String> {
    let input = input.trim();

    let prefix = "<<call ";
    let suffix = ">>";

    if !input.starts_with(prefix) || !input.ends_with(suffix) {
        return Err("invalid tool call format".to_string());
    }

    let inner = &input[prefix.len()..input.len() - suffix.len()];

    let (name, json) = inner
        .split_once(' ')
        .ok_or_else(|| "missing tool arguments".to_string())?;

    if name.is_empty() {
        return Err("missing tool name".to_string());
    }
    /// Decodes one or more compact tool calls from separate lines.
pub fn decode_tool_calls(input: &str) -> Result<Vec<CompactToolCall>, String> {
    let mut calls = Vec::new();

    for line in input.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        calls.push(decode_tool_call(line)?);
    }

    if calls.is_empty() {
        return Err("no tool calls found".to_string());
    }

    Ok(calls)
}

    let arguments: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(json)
            .map_err(|_| "invalid tool arguments".to_string())?;

    Ok(CompactToolCall {
        name: name.to_string(),
        arguments,
    })
}
pub fn decode_tool_calls(input: &str) -> Result<Vec<CompactToolCall>, String> {
    let mut calls = Vec::new();

    for line in input.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        calls.push(decode_tool_call(line)?);
    }

    if calls.is_empty() {
        return Err("no tool calls found".to_string());
    }

    Ok(calls)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_valid_tool_call() {
        let input =
            r#"<<call create_calendar_event {"title":"Meeting","duration_min":30}>>"#;

        let call = decode_tool_call(input).unwrap();

        assert_eq!(call.name, "create_calendar_event");
        assert_eq!(call.arguments["title"], "Meeting");
        assert_eq!(call.arguments["duration_min"], 30);
    }
    #[test]
    fn rejects_malformed_call() {
        let input = r#"<<call create_calendar_event {"title":"Meeting"}"#;

        assert!(decode_tool_call(input).is_err());
    }
    #[test]
fn decodes_multiple_tool_calls() {
    let input = r#"
<<call create_calendar_event {"title":"Meeting","start":"2026-10-05T15:00:00+05:30"}>>
<<call create_calendar_event {"title":"Lunch","start":"2026-10-06T12:00:00+05:30"}>>
"#;

    let calls = decode_tool_calls(input).unwrap();

    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(calls[1].name, "create_calendar_event");
    assert_eq!(calls[0].arguments["title"], "Meeting");
    assert_eq!(calls[1].arguments["title"], "Lunch");
}
    #[test]
fn rejects_unknown_field() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![
            CompactParameter {
                name: "title".to_string(),
                kind: CompactType::String,
                required: true,
            },
        ],
    };

    let input =
        r#"<<call create_calendar_event {"title":"Meeting","unknown":"value"}>>"#;

    let call = decode_tool_call(input).unwrap();

    let result = validate_tool_call(&call, &tool);

    assert!(result.is_err());
}
#[test]
    fn decodes_realistic_calendar_event_call() {
        let input =
            r#"<<call create_calendar_event {"title":"Meeting","start":"2026-10-05T15:00:00+05:30"}>>"#;

        let call = decode_tool_call(input).unwrap();

        assert_eq!(call.name, "create_calendar_event");
        assert_eq!(call.arguments["title"], "Meeting");
        assert_eq!(
            call.arguments["start"],
            "2026-10-05T15:00:00+05:30"
        );
    }
}
pub fn validate_tool_call(
    call: &CompactToolCall,
    tool: &CompactTool,
) -> Result<(), String> {
    if call.name != tool.name {
        return Err(format!("unknown tool: {}", call.name));
    }

    // Reject arguments that are not defined by the tool.
    for argument_name in call.arguments.keys() {
        if !tool
            .parameters
            .iter()
            .any(|parameter| parameter.name == *argument_name)
        {
            return Err(format!("unknown field: {}", argument_name));
        }
    }

    for parameter in &tool.parameters {
        match call.arguments.get(&parameter.name) {
            Some(value) => match &parameter.kind {
                CompactType::String => {
                    if !value.is_string() {
                        return Err(format!("wrong type for {}", parameter.name));
                    }
                }

                CompactType::Integer => {
                    if !value.is_i64() {
                        return Err(format!("wrong type for {}", parameter.name));
                    }
                }

                CompactType::Boolean => {
                    if !value.is_boolean() {
                        return Err(format!("wrong type for {}", parameter.name));
                    }
                }

                CompactType::Array => {
                    if !value.is_array() {
                        return Err(format!("wrong type for {}", parameter.name));
                    }
                }

                CompactType::Object => {
                    if !value.is_object() {
                        return Err(format!("wrong type for {}", parameter.name));
                    }
                }

                CompactType::Enum(values) => {
                    let value = value
                        .as_str()
                        .ok_or_else(|| {
                            format!("wrong type for {}", parameter.name)
                        })?;

                    if !values.iter().any(|v| v == value) {
                        return Err(format!(
                            "invalid enum value for {}",
                            parameter.name
                        ));
                    }
                }
            },

            None if parameter.required => {
                return Err(format!(
                    "missing required field: {}",
                    parameter.name
                ));
            }

            None => {}
        }
    }

    Ok(())
}
#[test]
fn rejects_unknown_tool() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![],
    };

    let call = CompactToolCall {
        name: "unknown_tool".to_string(),
        arguments: std::collections::BTreeMap::new(),
    };

    assert!(validate_tool_call(&call, &tool).is_err());
}

#[test]
fn rejects_missing_required_field() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![CompactParameter {
            name: "title".to_string(),
            kind: CompactType::String,
            required: true,
        }],
    };

    let call = CompactToolCall {
        name: "create_calendar_event".to_string(),
        arguments: std::collections::BTreeMap::new(),
    };

    assert!(validate_tool_call(&call, &tool).is_err());
}
#[test]
fn rejects_wrong_data_type() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![CompactParameter {
            name: "title".to_string(),
            kind: CompactType::String,
            required: true,
        }],
    };

    let mut arguments = std::collections::BTreeMap::new();
    arguments.insert(
        "title".to_string(),
        serde_json::json!(123),
    );

    let call = CompactToolCall {
        name: "create_calendar_event".to_string(),
        arguments,
    };

    assert!(validate_tool_call(&call, &tool).is_err());
}

#[test]
fn rejects_invalid_enum_value() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![CompactParameter {
            name: "visibility".to_string(),
            kind: CompactType::Enum(vec![
                "public".to_string(),
                "private".to_string(),
            ]),
            required: false,
        }],
    };

    let mut arguments = std::collections::BTreeMap::new();
    arguments.insert(
        "visibility".to_string(),
        serde_json::json!("secret"),
    );

    let call = CompactToolCall {
        name: "create_calendar_event".to_string(),
        arguments,
    };

    assert!(validate_tool_call(&call, &tool).is_err());
}
#[test]
fn accepts_array() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![CompactParameter {
            name: "attendees".to_string(),
            kind: CompactType::Array,
            required: false,
        }],
    };

    let mut arguments = std::collections::BTreeMap::new();
    arguments.insert(
        "attendees".to_string(),
        serde_json::json!(["Alice", "Bob"]),
    );

    let call = CompactToolCall {
        name: "create_calendar_event".to_string(),
        arguments,
    };

    assert!(validate_tool_call(&call, &tool).is_ok());
}
#[test]
fn accepts_nested_object() {
    let tool = CompactTool {
        name: "create_calendar_event".to_string(),
        parameters: vec![CompactParameter {
            name: "location".to_string(),
            kind: CompactType::Object,
            required: false,
        }],
    };

    let mut arguments = std::collections::BTreeMap::new();
    arguments.insert(
        "location".to_string(),
        serde_json::json!({
            "room": "A101",
            "building": "Main"
        }),
    );

    let call = CompactToolCall {
        name: "create_calendar_event".to_string(),
        arguments,
    };

    assert!(validate_tool_call(&call, &tool).is_ok());
// Test encoding directly from a JSON Schema.
    let schema = serde_json::json!({
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
            }
        },
        "required": ["title", "start"]
    });

    let schema_tool = encode_tool_schema(
        "create_calendar_event",
        &schema,
    )
    .expect("failed to encode tool schema");

    println!("\nSchema encoder:");
    println!("{}", serde_json::to_string(&schema_tool).unwrap());
}
#[test]
fn encodes_json_schema() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string"
            },
            "duration_min": {
                "type": "integer"
            }
        },
        "required": ["title"]
    });

    let tool = encode_tool_schema(
        "create_calendar_event",
        &schema,
    )
    .unwrap();

    assert_eq!(tool.name, "create_calendar_event");
    assert_eq!(tool.parameters.len(), 2);

    assert!(tool.parameters.iter().any(|p| {
        p.name == "title" && p.required
    }));

    assert!(tool.parameters.iter().any(|p| {
        p.name == "duration_min" && !p.required
    }));
}