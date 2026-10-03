use crate::error::CompactError;
use crate::types::ToolDef;
use serde_json::Value;

/// Validate decoded arguments against the tool's original JSON Schema.
///
/// Fails closed: a missing required field, an unknown field, a type mismatch or
/// an enum violation is an error, never a corrected or guessed call.
pub(crate) fn validate_arguments(args: &Value, def: &ToolDef) -> Result<(), CompactError> {
    let bad = |reason: String| CompactError::InvalidArguments { tool: def.name.clone(), reason };

    let Some(schema) = &def.parameters else {
        return match args.as_object() {
            Some(m) if m.is_empty() => Ok(()),
            _ => Err(bad("tool takes no arguments".into())),
        };
    };
    check(args, schema, &def.name, "arguments")
}

fn check(value: &Value, schema: &Value, tool: &str, path: &str) -> Result<(), CompactError> {
    let bad = |reason: String| CompactError::InvalidArguments { tool: tool.to_string(), reason };

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.iter().any(|a| a == value) {
            return Err(bad(format!("{path}: value {value} is not one of the allowed values")));
        }
        return Ok(());
    }

    let Some(ty) = schema.get("type").and_then(Value::as_str) else {
        return Ok(());
    };

    match ty {
        "object" => {
            let Some(map) = value.as_object() else {
                return Err(bad(format!("{path}: expected an object")));
            };
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for r in &required {
                if !map.contains_key(*r) {
                    return Err(bad(format!("{path}: missing required field `{r}`")));
                }
            }
            let props = schema.get("properties").and_then(Value::as_object);
            for (k, v) in map {
                match props.and_then(|p| p.get(k)) {
                    Some(sub) => check(v, sub, tool, &format!("{path}.{k}"))?,
                    None => return Err(bad(format!("{path}: unknown field `{k}`"))),
                }
            }
            Ok(())
        }
        "array" => {
            let Some(items) = value.as_array() else {
                return Err(bad(format!("{path}: expected an array")));
            };
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    check(item, item_schema, tool, &format!("{path}[{i}]"))?;
                }
            }
            Ok(())
        }
        "string" => {
            let Some(s) = value.as_str() else {
                return Err(bad(format!("{path}: expected a string")));
            };
            if schema.get("format").and_then(Value::as_str) == Some("date-time")
                && !looks_like_datetime(s)
            {
                return Err(bad(format!("{path}: `{s}` is not an ISO 8601 date-time")));
            }
            Ok(())
        }
        "integer" => match value {
            Value::Number(n) if n.is_i64() || n.is_u64() => Ok(()),
            _ => Err(bad(format!("{path}: expected an integer"))),
        },
        "number" => match value {
            Value::Number(_) => Ok(()),
            _ => Err(bad(format!("{path}: expected a number"))),
        },
        "boolean" => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(bad(format!("{path}: expected a boolean"))),
        },
        "null" => match value {
            Value::Null => Ok(()),
            _ => Err(bad(format!("{path}: expected null"))),
        },
        _ => Ok(()),
    }
}

/// Structural check for an ISO 8601 date-time: `YYYY-MM-DDTHH:MM` at minimum.
/// Deliberately lenient about the zone suffix, strict enough to reject prose
/// like "tomorrow 10am".
fn looks_like_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let digit = |i: usize| b[i].is_ascii_digit();
    digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && b[4] == b'-'
        && digit(5)
        && digit(6)
        && b[7] == b'-'
        && digit(8)
        && digit(9)
        && (b[10] == b'T' || b[10] == b't' || b[10] == b' ')
        && digit(11)
        && digit(12)
        && b[13] == b':'
        && digit(14)
        && digit(15)
}
