use crate::{DecodeError, ToolDef};
use serde_json::{Map, Value};

#[allow(dead_code)]
pub fn validate_call(tool: &ToolDef, arguments: &Map<String, Value>) -> Result<(), DecodeError> {
    let Some(parameters) = tool.parameters.as_ref() else {
        if arguments.is_empty() {
            return Ok(());
        }
        return Err(DecodeError::InvalidArguments {
            tool: tool.name.clone(),
            path: "/".to_string(),
            reason: "tool accepts no arguments; got non-empty object".to_string(),
        });
    };

    let root = parameters.as_object().ok_or_else(|| {
        DecodeError::UnsupportedSchema(crate::EncodeError::Unsupported {
            tool: tool.name.clone(),
            path: "/".to_string(),
            reason: "root parameters must be an object".to_string(),
        })
    })?;

    let empty: Map<String, Value> = Map::new();
    let properties = root
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for key in arguments.keys() {
        if !properties.contains_key(key) {
            return Err(DecodeError::InvalidArguments {
                tool: tool.name.clone(),
                path: format!("/{key}"),
                reason: "unknown key".to_string(),
            });
        }
    }
    for key in required {
        if !arguments.contains_key(&key) {
            return Err(DecodeError::InvalidArguments {
                tool: tool.name.clone(),
                path: format!("/{key}"),
                reason: "missing required field".to_string(),
            });
        }
    }
    for (key, value) in arguments {
        let schema = properties
            .get(key)
            .ok_or_else(|| DecodeError::InvalidArguments {
                tool: tool.name.clone(),
                path: format!("/{key}"),
                reason: "unknown field".to_string(),
            })?;
        validate_node(schema, value, &tool.name, &format!("/{key}"))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_node(
    schema: &Value,
    value: &Value,
    tool: &str,
    path: &str,
) -> Result<(), DecodeError> {
    let object = schema
        .as_object()
        .ok_or_else(|| DecodeError::InvalidArguments {
            tool: tool.to_string(),
            path: path.to_string(),
            reason: "schema node must be an object".to_string(),
        })?;

    if let Some(types) = object.get("type") {
        if let Some(type_name) = types.as_str() {
            if !type_matches(type_name, value) {
                return Err(DecodeError::InvalidArguments {
                    tool: tool.to_string(),
                    path: path.to_string(),
                    reason: format!("expected {type_name}, got {value}"),
                });
            }
        } else if let Some(array) = types.as_array() {
            let allowed = array.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            let matches = allowed.iter().any(|name| type_matches(name, value));
            if !matches {
                return Err(DecodeError::InvalidArguments {
                    tool: tool.to_string(),
                    path: path.to_string(),
                    reason: format!("expected one of {allowed:?}, got {value}"),
                });
            }
        }
    }

    if let Some(enum_values) = object.get("enum").and_then(Value::as_array) {
        let matches = enum_values.iter().any(|candidate| candidate == value);
        if !matches {
            return Err(DecodeError::InvalidArguments {
                tool: tool.to_string(),
                path: path.to_string(),
                reason: format!("value not in enum: {value}"),
            });
        }
    }

    if let Some(format) = object.get("format").and_then(Value::as_str)
        && !matches_format(format, value)
    {
        return Err(DecodeError::InvalidArguments {
            tool: tool.to_string(),
            path: path.to_string(),
            reason: format!("invalid format {format} for {value}"),
        });
    }

    if let Some(items_schema) = object.get("items")
        && let Value::Array(array) = value
    {
        for (idx, item) in array.iter().enumerate() {
            validate_node(items_schema, item, tool, &format!("{path}/{idx}"))?;
        }
    }

    if let Some(properties) = object.get("properties").and_then(Value::as_object)
        && let Value::Object(obj) = value
    {
        for (key, nested) in obj {
            if let Some(schema) = properties.get(key.as_str()) {
                validate_node(schema, nested, tool, &format!("{path}/{key}"))?;
            }
        }
    }

    Ok(())
}

fn type_matches(type_name: &str, value: &Value) -> bool {
    match (type_name, value) {
        ("string", Value::String(_)) => true,
        ("integer", Value::Number(n)) => n.is_i64() || n.is_u64(),
        ("number", Value::Number(_)) => true,
        ("boolean", Value::Bool(_)) => true,
        ("null", Value::Null) => true,
        ("array", Value::Array(_)) => true,
        ("object", Value::Object(_)) => true,
        _ => false,
    }
}

fn matches_format(format: &str, value: &Value) -> bool {
    let Value::String(s) = value else {
        return false;
    };
    match format {
        "date-time" => s.contains('T') || s.contains('t'),
        "date" => s.contains('-') && s.len() >= 8,
        "time" => {
            s.contains(':')
                && (s.contains('+') || s.contains('Z') || s.contains('z') || s.contains('-'))
        }
        "email" => s.contains('@'),
        "uri" => s.starts_with("http://") || s.starts_with("https://") || s.starts_with("mailto:"),
        "uuid" => s.len() >= 32,
        _ => true,
    }
}
