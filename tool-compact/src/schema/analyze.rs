use super::{
    CanonicalTool, MAX_DEPTH, MAX_PROPERTIES, MAX_TOOLS, Property, SchemaKind, SchemaNode,
};
use crate::{CompactError, Result, ToolDef};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Analyze every supplied tool, returning an error before any unsafe compaction.
pub fn analyze_tools(tools: &[ToolDef]) -> Result<Vec<CanonicalTool>> {
    if tools.len() > MAX_TOOLS {
        return Err(CompactError::LimitExceeded("tool count"));
    }
    let mut names = BTreeSet::new();
    tools
        .iter()
        .map(|tool| {
            let name = &tool.function.name;
            if !names.insert(name) {
                return Err(invalid(name, "duplicate tool name"));
            }
            if tool.kind != "function" {
                return Err(unsupported(name, "$", "tool type"));
            }
            if !safe_identifier(name) {
                return Err(unsupported(name, "$", "tool name"));
            }
            for (path, extra) in [("$", &tool.extra), ("$.function", &tool.function.extra)] {
                if let Some(keyword) = extra.keys().next() {
                    return Err(unsupported(name, path, keyword));
                }
            }
            let parameters = match &tool.function.parameters {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let node = analyze(value, name, "$.parameters", 0)?;
                    if !matches!(node.kind, SchemaKind::Object { .. }) {
                        return Err(invalid(name, "parameters must be object"));
                    }
                    Some(node)
                }
            };
            Ok(CanonicalTool {
                name: name.clone(),
                description: tool.function.description.clone(),
                parameters,
            })
        })
        .collect()
}

fn analyze(value: &Value, tool: &str, path: &str, depth: usize) -> Result<SchemaNode> {
    if depth >= MAX_DEPTH {
        return Err(CompactError::LimitExceeded("schema depth"));
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid(tool, "schema must be object"))?;
    let kind_name = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported(tool, path, "type"))?;
    check_keywords(object, tool, path, kind_name)?;
    let kind = match kind_name {
        "string" => SchemaKind::String,
        "integer" => SchemaKind::Integer,
        "number" => SchemaKind::Number,
        "boolean" => SchemaKind::Boolean,
        "array" => {
            let items = object
                .get("items")
                .ok_or_else(|| invalid(tool, "array requires items"))?;
            SchemaKind::Array(Box::new(analyze(
                items,
                tool,
                &format!("{path}.items"),
                depth + 1,
            )?))
        }
        "object" => analyze_object(object, tool, path, depth)?,
        _ => return Err(unsupported(tool, path, "type")),
    };
    let enum_values = match object.get("enum") {
        None => None,
        Some(value) => {
            let values = value
                .as_array()
                .ok_or_else(|| invalid(tool, "enum must be array"))?;
            if values.is_empty() || values.iter().any(|value| !scalar_matches(&kind, value)) {
                return Err(invalid(
                    tool,
                    "enum must be nonempty and match primitive type",
                ));
            }
            Some(values.clone())
        }
    };
    Ok(SchemaNode {
        kind,
        description: metadata(object, "description", tool)?,
        format: metadata(object, "format", tool)?,
        enum_values,
    })
}

fn check_keywords(object: &Map<String, Value>, tool: &str, path: &str, kind: &str) -> Result<()> {
    for keyword in object.keys() {
        let supported = match keyword.as_str() {
            "type" | "description" => true,
            "enum" => matches!(kind, "string" | "integer" | "number" | "boolean"),
            "format" => kind == "string",
            "items" => kind == "array",
            "properties" | "required" | "additionalProperties" => kind == "object",
            _ => false,
        };
        if !supported {
            return Err(unsupported(tool, path, keyword));
        }
    }
    Ok(())
}

fn analyze_object(
    object: &Map<String, Value>,
    tool: &str,
    path: &str,
    depth: usize,
) -> Result<SchemaKind> {
    let empty = Map::new();
    let raw = match object.get("properties") {
        None => &empty,
        Some(value) => value
            .as_object()
            .ok_or_else(|| invalid(tool, "properties must be object"))?,
    };
    if raw.len() > MAX_PROPERTIES {
        return Err(CompactError::LimitExceeded("property count"));
    }
    let mut required = BTreeSet::new();
    if let Some(value) = object.get("required") {
        for entry in value
            .as_array()
            .ok_or_else(|| invalid(tool, "required must be array"))?
        {
            let name = entry
                .as_str()
                .ok_or_else(|| invalid(tool, "required entries must be strings"))?;
            if !raw.contains_key(name) || !required.insert(name) {
                return Err(invalid(tool, "invalid or duplicate required property"));
            }
        }
    }
    let additional_properties = match object.get("additionalProperties") {
        None | Some(Value::Bool(true)) => true,
        Some(Value::Bool(false)) => false,
        Some(_) => return Err(unsupported(tool, path, "additionalProperties")),
    };
    let mut properties = BTreeMap::new();
    for (name, value) in raw {
        if !safe_identifier(name) || name.contains(':') {
            return Err(unsupported(tool, path, "property name"));
        }
        properties.insert(
            name.clone(),
            Property {
                required: required.contains(name.as_str()),
                schema: analyze(value, tool, &format!("{path}.properties.{name}"), depth + 1)?,
            },
        );
    }
    Ok(SchemaKind::Object {
        properties,
        additional_properties,
    })
}

fn metadata(object: &Map<String, Value>, keyword: &str, tool: &str) -> Result<Option<String>> {
    object
        .get(keyword)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(tool, "metadata must be string"))
        })
        .transpose()
}

pub(crate) fn scalar_matches(kind: &SchemaKind, value: &Value) -> bool {
    match kind {
        SchemaKind::String => value.is_string(),
        SchemaKind::Boolean => value.is_boolean(),
        SchemaKind::Number => value.is_number(),
        SchemaKind::Integer => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        _ => false,
    }
}

pub(crate) fn safe_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-'))
}

fn invalid(tool: &str, reason: &str) -> CompactError {
    CompactError::InvalidSchema {
        tool: tool.into(),
        reason: reason.into(),
    }
}
fn unsupported(tool: &str, path: &str, keyword: &str) -> CompactError {
    CompactError::UnsupportedSchema {
        tool: tool.into(),
        path: path.into(),
        keyword: keyword.into(),
    }
}
