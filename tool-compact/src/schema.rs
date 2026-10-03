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

impl Shape {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn fields(&self) -> &[Field] {
        match self {
            Shape::Object { fields } => fields,
            _ => &[],
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::calendar;
    use crate::types::ToolDef;
    use serde_json::json;

    #[test]
    fn calendar_schema_is_supported() {
        let shape = classify(&calendar()).unwrap();
        let fields = shape.fields();
        assert!(fields.iter().any(|f| {
            f.name == "title" && f.required && f.shape == Shape::Scalar(Scalar::Str)
        }));
        assert!(
            fields
                .iter()
                .any(|f| f.name == "duration_min" && !f.required)
        );
        assert!(fields.iter().any(|f| {
            f.name == "visibility"
                && matches!(&f.shape, Shape::Enum(v) if v.as_slice() == ["public", "private"])
        }));
        assert!(
            fields
                .iter()
                .any(|f| f.name == "attendees" && matches!(f.shape, Shape::Array(_)))
        );
    }

    #[test]
    fn ref_schema_is_unsupported_and_not_simplified() {
        let tool = ToolDef {
            name: "lookup".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"id": {"$ref": "#/$defs/Id"}}
            })),
        };
        let err = classify(&tool).unwrap_err();
        assert!(matches!(err, CompactError::UnsupportedSchema { feature, .. } if feature == "$ref"));
    }
}
