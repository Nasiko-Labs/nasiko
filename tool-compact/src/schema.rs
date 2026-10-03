//! Which JSON Schema features the compact grammar can carry.
//!
//! A feature outside this set is [`CompactError::UnsupportedSchema`]. The walker
//! stops at the first one. It does not drop the feature and keep going.

use serde_json::Value;

use crate::types::{CompactError, ToolDef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scalar {
    Str,
    Int,
    Num,
    Bool,
    DateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Shape {
    Scalar(Scalar),
    Enum(Vec<String>),
    Array(Box<Shape>),
    Object { fields: Vec<Field> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub name: String,
    pub required: bool,
    pub shape: Shape,
}

pub(crate) fn check(shape: &Shape, value: &Value) -> Result<(), crate::types::ArgumentFault> {
    match shape {
        Shape::Object { fields } => check_object(fields, value, ""),
        other => check_value(other, value, ""),
    }
}

fn check_object(
    fields: &[Field],
    value: &Value,
    field_name: &str,
) -> Result<(), crate::types::ArgumentFault> {
    use crate::types::ArgumentFault;
    let Some(object) = value.as_object() else {
        return Err(ArgumentFault::WrongType {
            field: field_name.to_string(),
        });
    };
    for field in fields {
        if field.required && !object.contains_key(&field.name) {
            return Err(ArgumentFault::MissingField(field.name.clone()));
        }
    }
    for (key, child) in object {
        let Some(field) = fields.iter().find(|field| field.name == *key) else {
            return Err(ArgumentFault::WrongType { field: key.clone() });
        };
        check_value(&field.shape, child, key)?;
    }
    Ok(())
}

fn check_value(
    shape: &Shape,
    value: &Value,
    field: &str,
) -> Result<(), crate::types::ArgumentFault> {
    use crate::types::ArgumentFault;
    let wrong = || ArgumentFault::WrongType {
        field: field.to_string(),
    };
    match shape {
        Shape::Scalar(Scalar::Str | Scalar::DateTime) => {
            value.as_str().map(|_| ()).ok_or_else(wrong)
        }
        Shape::Scalar(Scalar::Int) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|n| n as i64))
            .map(|_| ())
            .ok_or_else(wrong),
        Shape::Scalar(Scalar::Num) => value.as_number().map(|_| ()).ok_or_else(wrong),
        Shape::Scalar(Scalar::Bool) => value.as_bool().map(|_| ()).ok_or_else(wrong),
        Shape::Enum(allowed) => {
            let Some(text) = value.as_str() else {
                return Err(wrong());
            };
            if allowed.iter().any(|item| item == text) {
                Ok(())
            } else {
                Err(ArgumentFault::BadEnum {
                    field: field.to_string(),
                })
            }
        }
        Shape::Array(inner) => {
            let Some(items) = value.as_array() else {
                return Err(wrong());
            };
            for item in items {
                check_value(inner, item, field)?;
            }
            Ok(())
        }
        Shape::Object { fields } => check_object(fields, value, field),
    }
}

pub(crate) fn classify(tool: &ToolDef) -> Result<Shape, CompactError> {
    match &tool.parameters {
        None => Ok(Shape::Object { fields: Vec::new() }),
        Some(schema) => read_node(tool, schema, true),
    }
}

fn unsupported(tool: &ToolDef, feature: &str) -> CompactError {
    CompactError::UnsupportedSchema {
        name: tool.name.clone(),
        feature: feature.to_string(),
    }
}

fn read_node(tool: &ToolDef, node: &Value, root: bool) -> Result<Shape, CompactError> {
    let Some(obj) = node.as_object() else {
        return Err(unsupported(tool, "non-object schema"));
    };
    for key in ["$ref", "oneOf", "anyOf", "allOf", "not", "prefixItems"] {
        if obj.contains_key(key) {
            return Err(unsupported(tool, key));
        }
    }
    if let Some(extra) = obj.get("additionalProperties")
        && !extra.is_boolean()
    {
        return Err(unsupported(tool, "additionalProperties"));
    }
    if let Some(items) = obj.get("items")
        && items.is_array()
    {
        return Err(unsupported(tool, "tuple items"));
    }
    if let Some(format) = obj.get("format")
        && format.as_str() != Some("date-time")
    {
        return Err(unsupported(tool, "format"));
    }
    if let Some(values) = obj.get("enum") {
        let Some(entries) = values.as_array() else {
            return Err(unsupported(tool, "enum"));
        };
        if entries.iter().any(|entry| !entry.is_string()) {
            return Err(unsupported(tool, "enum"));
        }
    }

    if obj.contains_key("enum") {
        let entries = obj["enum"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Shape::Enum(entries));
    }

    let type_name = obj.get("type").and_then(Value::as_str);
    match type_name {
        Some("string") if obj.get("format").and_then(Value::as_str) == Some("date-time") => {
            Ok(Shape::Scalar(Scalar::DateTime))
        }
        Some("string") => Ok(Shape::Scalar(Scalar::Str)),
        Some("integer") => Ok(Shape::Scalar(Scalar::Int)),
        Some("number") => Ok(Shape::Scalar(Scalar::Num)),
        Some("boolean") => Ok(Shape::Scalar(Scalar::Bool)),
        Some("array") => {
            let items = obj.get("items").ok_or_else(|| unsupported(tool, "items"))?;
            Ok(Shape::Array(Box::new(read_node(tool, items, false)?)))
        }
        Some("object") | None if root || obj.contains_key("properties") => {
            let required = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut fields = Vec::new();
            if let Some(properties) = obj.get("properties").and_then(Value::as_object) {
                for (name, schema) in properties {
                    fields.push(Field {
                        required: required.iter().any(|item| item == name),
                        shape: read_node(tool, schema, false)?,
                        name: name.clone(),
                    });
                }
            }
            Ok(Shape::Object { fields })
        }
        _ => Err(unsupported(tool, "type")),
    }
}
