use std::collections::{BTreeSet, HashSet};

use serde_json::{Map, Value};

use crate::CompactError;

pub(crate) fn validate_supported(schema: &Value) -> Result<(), CompactError> {
    reject_references(schema, "$")?;
    jsonschema::options()
        .offline()
        .should_validate_formats(true)
        .build(schema)
        .map(|_| ())
        .map_err(|error| CompactError::UnsupportedSchema {
            path: "$".to_string(),
            feature: format!("invalid JSON Schema: {error}"),
        })
}

pub(crate) fn render_parameters(schema: &Value) -> Result<String, CompactError> {
    match schema {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("object") => {
            render_object(object, true)
        }
        _ => Ok(format!("({})", render_schema(schema)?)),
    }
}

fn render_schema(schema: &Value) -> Result<String, CompactError> {
    match schema {
        Value::Bool(true) => Ok("any".to_string()),
        Value::Bool(false) => Ok("never".to_string()),
        Value::Object(object) => render_schema_object(object),
        other => compact_json(other),
    }
}

fn render_schema_object(object: &Map<String, Value>) -> Result<String, CompactError> {
    let description = object.get("description");
    let mut consumed = HashSet::from(["description"]);
    let mut rendered = match object.get("type") {
        Some(Value::String(kind)) => {
            consumed.insert("type");
            match kind.as_str() {
                "object" => return render_object(object, false),
                "array" => {
                    consumed.insert("items");
                    let items = object
                        .get("items")
                        .map(render_schema)
                        .transpose()?
                        .unwrap_or_else(|| "any".to_string());
                    format!("[{items}]")
                }
                "string" => {
                    if let Some(format) = object.get("format").and_then(Value::as_str)
                        && let Some(familiar) = familiar_format(format)
                    {
                        consumed.insert("format");
                        familiar.to_string()
                    } else {
                        "str".to_string()
                    }
                }
                "integer" => "int".to_string(),
                "number" => "num".to_string(),
                "boolean" => "bool".to_string(),
                "null" => "null".to_string(),
                _ => compact_json(&Value::Object(object.clone()))?,
            }
        }
        Some(Value::Array(kinds)) => {
            consumed.insert("type");
            kinds
                .iter()
                .map(|kind| {
                    kind.as_str().map_or_else(
                        || compact_json(kind),
                        |kind| Ok(familiar_type(kind).to_string()),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("|")
        }
        Some(_) => return compact_json(&Value::Object(object.clone())),
        None => {
            if let Some(values) = object.get("enum") {
                consumed.insert("enum");
                format!("enum{}", compact_json(values)?)
            } else if let Some(value) = object.get("const") {
                consumed.insert("const");
                format!("const({})", compact_json(value)?)
            } else {
                let mut visible = object.clone();
                visible.remove("description");
                let mut rendered = compact_json(&Value::Object(visible))?;
                append_description(&mut rendered, description)?;
                return Ok(rendered);
            }
        }
    };

    if let Some(values) = object.get("enum")
        && !consumed.contains("enum")
    {
        consumed.insert("enum");
        let literals = if let Some(values) = values.as_array() {
            values
                .iter()
                .map(compact_json)
                .collect::<Result<Vec<_>, _>>()?
                .join("|")
        } else {
            format!("enum{}", compact_json(values)?)
        };
        if enum_implies_type(values, object.get("type")) {
            rendered = literals;
        } else {
            rendered.push_str(" enum[");
            rendered.push_str(&literals);
            rendered.push(']');
        }
    }
    if let Some(format) = object.get("format")
        && !consumed.contains("format")
    {
        consumed.insert("format");
        rendered.push_str(" format=");
        rendered.push_str(&compact_json(format)?);
    }
    append_remaining_constraints(&mut rendered, object, &consumed)?;
    append_description(&mut rendered, description)?;
    Ok(rendered)
}

fn render_object(object: &Map<String, Value>, root: bool) -> Result<String, CompactError> {
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = required_names(object.get("required"));
    let mut fields = Vec::with_capacity(properties.len());
    for (name, schema) in properties {
        let optional = if required.contains(&name) { "" } else { "?" };
        fields.push(format!(
            "{}{optional}:{}",
            render_name(&name)?,
            render_schema(&schema)?
        ));
    }

    let mut rendered = if root {
        format!("({})", fields.join(","))
    } else {
        format!("{{{}}}", fields.join(","))
    };
    if let Some(additional) = object.get("additionalProperties") {
        match additional {
            Value::Bool(true) => {}
            Value::Bool(false) => rendered.push_str(" extra=false"),
            schema => {
                rendered.push_str(" extra:");
                rendered.push_str(&render_schema(schema)?);
            }
        }
    }

    let property_names = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let unknown_required = required
        .iter()
        .filter(|name| !property_names.contains(name))
        .cloned()
        .collect::<Vec<_>>();
    let mut consumed = HashSet::from(["type", "properties", "description", "additionalProperties"]);
    if unknown_required.is_empty() {
        consumed.insert("required");
    }
    append_remaining_constraints(&mut rendered, object, &consumed)?;
    append_description(&mut rendered, object.get("description"))?;
    Ok(rendered)
}

fn append_remaining_constraints(
    rendered: &mut String,
    object: &Map<String, Value>,
    consumed: &HashSet<&str>,
) -> Result<(), CompactError> {
    let remaining = object
        .iter()
        .filter(|(key, _)| !consumed.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Map<_, _>>();
    if !remaining.is_empty() {
        rendered.push_str(" where ");
        rendered.push_str(&compact_json(&Value::Object(remaining))?);
    }
    Ok(())
}

fn append_description(
    rendered: &mut String,
    description: Option<&Value>,
) -> Result<(), CompactError> {
    if let Some(description) = description {
        rendered.push_str(&compact_json(description)?);
    }
    Ok(())
}

fn required_names(required: Option<&Value>) -> BTreeSet<String> {
    required
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn render_name(name: &str) -> Result<String, CompactError> {
    if !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        Ok(name.to_string())
    } else {
        compact_json(&Value::String(name.to_string()))
    }
}

fn compact_json(value: &Value) -> Result<String, CompactError> {
    serde_json::to_string(value)
        .map_err(|error| CompactError::InvalidCompactEncoding(error.to_string()))
}

fn familiar_type(kind: &str) -> &str {
    match kind {
        "string" => "str",
        "integer" => "int",
        "number" => "num",
        "boolean" => "bool",
        other => other,
    }
}

fn familiar_format(format: &str) -> Option<&'static str> {
    match format {
        "date-time" => Some("datetime"),
        "date" => Some("date"),
        "time" => Some("time"),
        "email" => Some("email"),
        "uri" => Some("uri"),
        "uuid" => Some("uuid"),
        _ => None,
    }
}

fn enum_implies_type(values: &Value, schema_type: Option<&Value>) -> bool {
    let (Some(values), Some(schema_type)) =
        (values.as_array(), schema_type.and_then(Value::as_str))
    else {
        return false;
    };
    values.iter().all(|value| match schema_type {
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    })
}

fn reject_references(value: &Value, path: &str) -> Result<(), CompactError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}/{key}");
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                    return Err(CompactError::UnsupportedSchema {
                        path: child_path,
                        feature: key.clone(),
                    });
                }
                reject_references(child, &child_path)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_references(child, &format!("{path}/{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}
