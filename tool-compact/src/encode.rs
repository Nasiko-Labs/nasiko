use crate::{CompactError, CompactTools, Result, ToolDef};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// Encode JSON Schema tool definitions into a compact, prompt-ready grammar.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut names = HashSet::new();
    let mut lines = Vec::with_capacity(tools.len() + 1);
    for tool in tools {
        if tool.name.trim().is_empty()
            || !tool
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            || !names.insert(tool.name.as_str())
        {
            return Err(CompactError::InvalidToolDefinition(format!(
                "empty, duplicate, or unrepresentable tool name '{}'",
                tool.name
            )));
        }
        let args = match &tool.parameters {
            None | Some(Value::Null) => "{}".to_owned(),
            Some(schema) => render_object(schema, &tool.name)?,
        };
        let description = tool
            .description
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut line = format!("{}{}", tool.name, args);
        if !description.is_empty() {
            line.push_str(" - ");
            line.push_str(&description);
        }
        lines.push(line);
    }
    lines.push("To call a tool, emit: <<call TOOL_NAME JSON_OBJECT>>".to_owned());
    Ok(CompactTools {
        definitions: lines.join("\n"),
    })
}

fn render_object(schema: &Value, path: &str) -> Result<String> {
    let obj = schema.as_object().ok_or_else(|| {
        CompactError::InvalidToolDefinition(format!(
            "{path}: parameters must be a JSON Schema object"
        ))
    })?;
    ensure_supported(obj, path)?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err(CompactError::UnsupportedSchema(format!(
            "{path}: tool parameters must explicitly have type object"
        )));
    }
    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if obj.contains_key("properties") && obj["properties"].as_object().is_none() {
        return Err(CompactError::InvalidToolDefinition(format!(
            "{path}.properties must be an object"
        )));
    }
    let required = required_properties(obj, path)?;
    if required.iter().any(|r| !properties.contains_key(*r)) {
        return Err(CompactError::InvalidToolDefinition(format!(
            "{path}.required names a property not present in properties"
        )));
    }
    let mut fields = Vec::new();
    for (name, field) in properties {
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err(CompactError::UnsupportedSchema(format!(
                "{path}.properties contains an unrepresentable field name"
            )));
        }
        let field_obj = field.as_object().ok_or_else(|| {
            CompactError::InvalidToolDefinition(format!("{path}.{name}: schema must be an object"))
        })?;
        let ty = render_type(&field, &format!("{path}.{name}"))?;
        fields.push(format_field(
            &name,
            if required.contains(name.as_str()) {
                ""
            } else {
                "?"
            },
            &ty,
            field_obj.get("description"),
        ));
    }
    if !obj.contains_key("additionalProperties") {
        fields.push("...:any".into());
    }
    Ok(format!("({})", fields.join(", ")))
}

fn ensure_supported(obj: &Map<String, Value>, path: &str) -> Result<()> {
    for key in ["description", "title"] {
        if obj.get(key).is_some_and(|value| !value.is_string()) {
            return Err(CompactError::InvalidToolDefinition(format!(
                "{path}.{key} must be a string"
            )));
        }
    }
    for key in [
        "oneOf",
        "anyOf",
        "allOf",
        "$ref",
        "$dynamicRef",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "dependentSchemas",
        "unevaluatedProperties",
        "contains",
    ] {
        if obj.contains_key(key) {
            return Err(CompactError::UnsupportedSchema(format!("{path}.{key}")));
        }
    }
    if obj
        .get("additionalProperties")
        .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err(CompactError::UnsupportedSchema(format!(
            "{path}.additionalProperties (only false is supported)"
        )));
    }
    for key in [
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
        "uniqueItems",
        "multipleOf",
    ] {
        if obj.contains_key(key) {
            return Err(CompactError::UnsupportedSchema(format!("{path}.{key}")));
        }
    }
    for key in obj.keys() {
        if ![
            "type",
            "properties",
            "required",
            "items",
            "enum",
            "format",
            "description",
            "title",
            "additionalProperties",
            "$schema",
            "$id",
            "$comment",
        ]
        .contains(&key.as_str())
        {
            return Err(CompactError::UnsupportedSchema(format!("{path}.{key}")));
        }
    }
    Ok(())
}

fn required_properties<'a>(obj: &'a Map<String, Value>, path: &str) -> Result<HashSet<&'a str>> {
    let Some(required) = obj.get("required") else {
        return Ok(HashSet::new());
    };
    let array = required.as_array().ok_or_else(|| {
        CompactError::InvalidToolDefinition(format!("{path}.required must be an array"))
    })?;
    array
        .iter()
        .map(|name| {
            name.as_str().ok_or_else(|| {
                CompactError::InvalidToolDefinition(format!(
                    "{path}.required entries must be strings"
                ))
            })
        })
        .collect()
}

fn render_type(schema: &Value, path: &str) -> Result<String> {
    let obj = schema.as_object().ok_or_else(|| {
        CompactError::InvalidToolDefinition(format!("{path}: schema must be an object"))
    })?;
    ensure_supported(obj, path)?;
    if let Some(en) = obj.get("enum") {
        let vals = en.as_array().ok_or_else(|| {
            CompactError::InvalidToolDefinition(format!("{path}.enum must be an array"))
        })?;
        if vals.is_empty()
            || vals.iter().any(|v| !v.is_string())
            || obj
                .get("type")
                .is_some_and(|v| v.as_str() != Some("string"))
        {
            return Err(CompactError::UnsupportedSchema(format!(
                "{path}: only non-empty string enums are supported"
            )));
        }
        if vals.iter().filter_map(Value::as_str).any(|s| {
            s.is_empty()
                || !s
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        }) {
            return Err(CompactError::UnsupportedSchema(format!(
                "{path}: enum values cannot be represented in compact syntax"
            )));
        }
        return Ok(vals
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("|"));
    }
    let ty = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
        CompactError::UnsupportedSchema(format!("{path}: missing or non-string type"))
    })?;
    match ty {
        "string" => match obj
            .get("format")
            .map(|v| {
                v.as_str().ok_or_else(|| {
                    CompactError::InvalidToolDefinition(format!("{path}.format must be a string"))
                })
            })
            .transpose()?
            .unwrap_or("")
        {
            "" => Ok("str".into()),
            "date-time" => Ok("datetime".into()),
            "date" => Ok("date".into()),
            "time" => Ok("time".into()),
            "email" => Ok("email".into()),
            "uri" => Ok("uri".into()),
            "uuid" => Ok("uuid".into()),
            f => Err(CompactError::UnsupportedSchema(format!(
                "{path}.format={f}"
            ))),
        },
        "integer" if !obj.contains_key("format") => Ok("int".into()),
        "number" if !obj.contains_key("format") => Ok("number".into()),
        "boolean" if !obj.contains_key("format") => Ok("bool".into()),
        "integer" | "number" | "boolean" => Err(CompactError::UnsupportedSchema(format!(
            "{path}.format is only supported for strings"
        ))),
        "array" => {
            let items = obj.get("items").ok_or_else(|| {
                CompactError::InvalidToolDefinition(format!("{path}.items is required"))
            })?;
            Ok(format!("[{}]", render_type(items, &format!("{path}[]"))?))
        }
        "object" => {
            let properties = obj
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    CompactError::UnsupportedSchema(format!(
                        "{path}: nested objects require properties"
                    ))
                })?;
            let required = required_properties(obj, path)?;
            if required.iter().any(|r| !properties.contains_key(*r)) {
                return Err(CompactError::InvalidToolDefinition(format!(
                    "{path}.required names a property not present in properties"
                )));
            }
            let mut fields = Vec::new();
            for (key, value) in properties {
                if !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                {
                    return Err(CompactError::UnsupportedSchema(format!(
                        "{path}.properties contains an unrepresentable field name"
                    )));
                }
                let child = value.as_object().ok_or_else(|| {
                    CompactError::InvalidToolDefinition(format!(
                        "{path}.{key}: schema must be an object"
                    ))
                })?;
                fields.push(format_field(
                    key,
                    if required.contains(key.as_str()) {
                        ""
                    } else {
                        "?"
                    },
                    &render_type(value, &format!("{path}.{key}"))?,
                    child.get("description"),
                ));
            }
            if !obj.contains_key("additionalProperties") {
                fields.push("...:any".into());
            }
            Ok(format!("{{{}}}", fields.join(", ")))
        }
        other => Err(CompactError::UnsupportedSchema(format!(
            "{path}.type={other}"
        ))),
    }
}

fn format_field(name: &str, optional: &str, ty: &str, description: Option<&Value>) -> String {
    let mut field = format!("{name}{optional}:{ty}");
    if let Some(description) = description
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        field.push(' ');
        field.push_str(&serde_json::to_string(description).unwrap_or_else(|_| "\"\"".into()));
    }
    field
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_required_optional_arrays_nested_enums_formats_and_descriptions() {
        let tool = ToolDef {
            name: "create_event".into(),
            description: Some("Create an event.".into()),
            parameters: Some(json!({"type":"object", "properties":{
                "title":{"type":"string", "description":"Event title"},
                "when":{"type":"string", "format":"date-time"},
                "email":{"type":"string", "format":"email"},
                "visibility":{"type":"string", "enum":["public","private"]},
                "labels":{"type":"array", "items":{"type":"string"}},
                "metadata":{"type":"object", "properties":{"priority":{"type":"integer"}}, "required":["priority"]}
            }, "required":["title","when"]})),
        };
        let out = encode_tools(&[tool]).unwrap().definitions;
        assert!(out.contains("title:str \"Event title\""));
        assert!(out.contains("when:datetime"));
        assert!(out.contains("email?:email"));
        assert!(out.contains("visibility?:public|private"));
        assert!(out.contains("labels?:[str]"));
        assert!(out.contains("...:any"));
        assert!(out.contains("metadata?:{priority:int, ...:any}"));
        assert!(out.contains(" - Create an event."));
    }

    #[test]
    fn rejects_schema_features_whose_meaning_cannot_be_represented() {
        for schema in [
            json!({"type":"object", "properties":{"x":{"oneOf":[{"type":"string"},{"type":"integer"}]}}}),
            json!({"properties":{"x":{"type":"string"}}}),
            json!({"type":"object", "properties":{"x":{"type":"string", "minLength":2}}}),
            json!({"type":"object", "properties":{"x":{"type":"integer", "enum":["1"]}}}),
        ] {
            let tool = ToolDef {
                name: "x".into(),
                description: None,
                parameters: Some(schema),
            };
            assert!(encode_tools(&[tool]).is_err());
        }
    }
}
