use crate::{
    error::EncodeError,
    format::{legend_for_definitions, prompt_for_definitions},
    types::{CompactTools, ToolDef},
};
use serde_json::{Map, Value};

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut seen = std::collections::HashSet::new();
    let mut definitions = Vec::new();
    for tool in tools {
        if !is_valid_name(&tool.name) {
            return Err(EncodeError::InvalidName(tool.name.clone()));
        }
        if !seen.insert(tool.name.clone()) {
            return Err(EncodeError::DuplicateTool(tool.name.clone()));
        }
        definitions.push(render_tool(tool)?);
    }

    let definitions_text = definitions.join("\n");
    let prompt = prompt_for_definitions(&definitions_text);
    Ok(CompactTools::new(definitions_text, prompt))
}

fn render_tool(tool: &ToolDef) -> Result<String, EncodeError> {
    let name = tool.name.clone();
    let args = tool.parameters.clone().unwrap_or(Value::Object(Map::new()));
    let root = match args {
        Value::Object(map) => map,
        other => {
            return Err(EncodeError::Unsupported {
                tool: tool.name.clone(),
                path: "/".to_string(),
                reason: format!("root parameters must be an object, found {other}"),
            });
        }
    };

    if root.is_empty() {
        return Ok(name.to_string());
    }

    let empty: Map<String, Value> = Map::new();
    let properties = root
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let required = root
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let required_set: std::collections::HashSet<_> = required.iter().copied().collect();
    let mut ordered = Vec::new();
    for key in properties.keys() {
        ordered.push(key.clone());
    }

    ordered.sort();
    let mut seen = Vec::new();
    for key in ordered {
        let property = properties
            .get(&key)
            .ok_or_else(|| EncodeError::Unsupported {
                tool: tool.name.clone(),
                path: format!("/properties/{key}"),
                reason: "missing property definition".to_string(),
            })?;
        let is_required = required_set.contains(key.as_str());
        let field_repr = compact_field(&key, property, is_required)?;
        seen.push(field_repr);
    }
    let args_part = seen.join(", ");
    let desc = tool.description.as_deref().unwrap_or("");
    if desc.is_empty() {
        Ok(format!("{name}({args_part})"))
    } else {
        Ok(format!("{name}({args_part}) - {desc}"))
    }
}

fn compact_field(key: &str, property: &Value, required: bool) -> Result<String, EncodeError> {
    let member = if required {
        format!("{key}:{}", compact_schema(property))
    } else {
        format!("{key}?:{}", compact_schema(property))
    };
    Ok(member)
}

fn compact_schema(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            if let Some(kind) = map.get("type") {
                match kind {
                    Value::String(s) => match s.as_str() {
                        "string" => {
                            if let Some(enum_values) = map.get("enum").and_then(Value::as_array) {
                                let vals = enum_values
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>();
                                if vals.is_empty() {
                                    return "str".to_string();
                                }
                                return vals.join("|");
                            }
                            if let Some(format) = map.get("format").and_then(Value::as_str) {
                                return format_name(format);
                            }
                            return "str".to_string();
                        }
                        "integer" => return "int".to_string(),
                        "number" => return "num".to_string(),
                        "boolean" => return "bool".to_string(),
                        "null" => return "null".to_string(),
                        "array" => {
                            let fallback = Value::String("any".to_string());
                            let item = map.get("items").unwrap_or(&fallback);
                            return format!("[{}]", compact_schema(item));
                        }
                        "object" => {
                            if map.get("properties").is_some() {
                                return "obj".to_string();
                            }
                            return "obj".to_string();
                        }
                        _ => return "any".to_string(),
                    },
                    Value::Array(items) if !items.is_empty() => {
                        let mut types = items.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>();
                        types.retain(|s| *s != "null");
                        if types.len() == 1 {
                            return compact_schema(&Value::String(types[0].to_string()));
                        }
                    }
                    _ => {}
                }
            }
            if let Some(enum_values) = map.get("enum").and_then(Value::as_array) {
                let vals = enum_values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>();
                if !vals.is_empty() {
                    return vals.join("|");
                }
            }
            if map.contains_key("properties") || map.contains_key("additionalProperties") {
                return "obj".to_string();
            }
            if let Some(items) = map.get("items") {
                return format!("[{}]", compact_schema(items));
            }
            "any".to_string()
        }
        Value::Array(items) => {
            let inner = items.iter().map(compact_schema).collect::<Vec<_>>();
            inner.join("|")
        }
        Value::String(s) => s.to_string(),
        _ => "any".to_string(),
    }
}

fn format_name(format: &str) -> String {
    match format {
        "date-time" => "datetime".to_string(),
        "date" => "date".to_string(),
        "time" => "time".to_string(),
        "email" => "email".to_string(),
        "uri" => "uri".to_string(),
        "uuid" => "uuid".to_string(),
        _ => "str".to_string(),
    }
}

fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    for ch in bytes {
        if ch.is_ascii_alphanumeric() || matches!(*ch, b'.' | b'_' | b'-') {
            continue;
        }
        return false;
    }
    true
}

pub fn compact_request_prompt(tools: &[ToolDef]) -> String {
    let _ = legend_for_definitions();
    let encoded =
        encode_tools(tools).unwrap_or_else(|_| CompactTools::new(String::new(), String::new()));
    encoded.prompt().to_string()
}
