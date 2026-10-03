use super::analyze::scalar_matches;
use super::{CanonicalTool, MAX_DEPTH, SchemaKind, SchemaNode};
use crate::{CompactError, Result};
use serde_json::Value;

pub(crate) fn validate(tool: &CanonicalTool, arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        return Err(invalid(&tool.name, "$", "root must be object"));
    }
    match &tool.parameters {
        Some(node) => validate_node(node, arguments, &tool.name, "$", 0),
        None if arguments
            .as_object()
            .is_some_and(|object| object.is_empty()) =>
        {
            Ok(())
        }
        None => Err(invalid(&tool.name, "$", "zero-argument tool requires {}")),
    }
}

fn validate_node(
    node: &SchemaNode,
    value: &Value,
    tool: &str,
    path: &str,
    depth: usize,
) -> Result<()> {
    if depth >= MAX_DEPTH {
        return Err(CompactError::LimitExceeded("argument depth"));
    }
    match &node.kind {
        SchemaKind::Object {
            properties,
            additional_properties,
        } => {
            let object = value
                .as_object()
                .ok_or_else(|| invalid(tool, path, "expected object"))?;
            for (name, property) in properties {
                let child_path = format!("{path}.{name}");
                match object.get(name) {
                    Some(child) => {
                        validate_node(&property.schema, child, tool, &child_path, depth + 1)?
                    }
                    None if property.required => {
                        return Err(invalid(tool, &child_path, "missing required property"));
                    }
                    None => {}
                }
            }
            if !additional_properties && object.keys().any(|name| !properties.contains_key(name)) {
                return Err(invalid(tool, path, "additional properties forbidden"));
            }
        }
        SchemaKind::Array(items) => {
            let array = value
                .as_array()
                .ok_or_else(|| invalid(tool, path, "expected array"))?;
            for (index, child) in array.iter().enumerate() {
                validate_node(items, child, tool, &format!("{path}[{index}]"), depth + 1)?;
            }
        }
        kind if !scalar_matches(kind, value) => {
            return Err(invalid(tool, path, "wrong primitive type"));
        }
        _ => {}
    }
    if let Some(values) = &node.enum_values
        && !values.iter().any(|allowed| enum_equal(allowed, value))
    {
        return Err(invalid(tool, path, "value not in enum"));
    }
    Ok(())
}

fn enum_equal(left: &Value, right: &Value) -> bool {
    if left == right {
        return true;
    }
    // JSON Schema compares numeric values, not their serialization. Avoid lossy
    // float conversion of two large integer values that differ by one.
    integer_float_equal(left, right) || integer_float_equal(right, left)
}

fn integer_float_equal(integer: &Value, floating: &Value) -> bool {
    if floating.is_i64() || floating.is_u64() {
        return false;
    }
    let Some(number) = floating.as_f64() else {
        return false;
    };
    if number.fract() != 0.0 {
        return false;
    }
    if let Some(value) = integer.as_i64() {
        return (-9223372036854775808.0..9223372036854775808.0).contains(&number)
            && number as i64 == value;
    }
    if let Some(value) = integer.as_u64() {
        return (0.0..18446744073709551616.0).contains(&number) && number as u64 == value;
    }
    false
}

fn invalid(tool: &str, path: &str, reason: &str) -> CompactError {
    CompactError::InvalidArguments {
        tool: tool.into(),
        path: path.into(),
        reason: reason.into(),
    }
}
