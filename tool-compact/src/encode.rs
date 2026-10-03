use serde_json::Value;
use std::collections::HashSet;

use crate::error::ToolCompactError;
use crate::types::{CompactTools, ToolDef};

pub const INSTRUCTION: &str = "To call a tool, emit: <<call name {json args}>>";

/// Encodes a slice of tool definitions into compact signatures and invocation instructions.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, ToolCompactError> {
    if tools.is_empty() {
        return Ok(CompactTools::new("", INSTRUCTION, INSTRUCTION));
    }

    let mut sig_lines = Vec::new();

    for tool in tools {
        let sig = encode_single_tool(tool)?;
        sig_lines.push(sig);
    }

    let signatures = sig_lines.join("\n");
    let full_prompt = format!("{signatures}\n{INSTRUCTION}");

    Ok(CompactTools::new(signatures, INSTRUCTION, full_prompt))
}

fn encode_single_tool(tool: &ToolDef) -> Result<String, ToolCompactError> {
    let name = &tool.function.name;

    let mut params_formatted = Vec::new();

    if let Some(ref params) = tool.function.parameters {
        if let Some(params_obj) = params.as_object() {
            check_for_unsupported_schema(params_obj, name)?;

            let required_set: HashSet<&str> = params_obj
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            if let Some(props_obj) = params_obj.get("properties").and_then(Value::as_object) {
                for (prop_name, prop_schema) in props_obj {
                    let is_required = required_set.contains(prop_name.as_str());
                    let type_str = encode_type_schema(prop_schema, name)?;

                    if is_required {
                        params_formatted.push(format!("{prop_name}:{type_str}"));
                    } else {
                        params_formatted.push(format!("{prop_name}?:{type_str}"));
                    }
                }
            }
        }
    }

    let params_str = params_formatted.join(", ");

    match &tool.function.description {
        Some(desc) if !desc.trim().is_empty() => {
            Ok(format!("{name}({params_str}) - {}", desc.trim()))
        }
        _ => Ok(format!("{name}({params_str})")),
    }
}

fn check_for_unsupported_schema(
    obj: &serde_json::Map<String, Value>,
    tool_name: &str,
) -> Result<(), ToolCompactError> {
    if obj.contains_key("oneOf")
        || obj.contains_key("anyOf")
        || obj.contains_key("allOf")
        || obj.contains_key("$ref")
    {
        return Err(ToolCompactError::UnsupportedSchema(format!(
            "Tool '{tool_name}' contains unsupported schema construct (oneOf/anyOf/allOf/$ref)"
        )));
    }

    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for prop in props.values() {
            if let Some(p_obj) = prop.as_object() {
                check_for_unsupported_schema(p_obj, tool_name)?;
            }
        }
    }

    if let Some(items) = obj.get("items").and_then(Value::as_object) {
        check_for_unsupported_schema(items, tool_name)?;
    }

    Ok(())
}

fn encode_type_schema(schema: &Value, tool_name: &str) -> Result<String, ToolCompactError> {
    if let Some(obj) = schema.as_object() {
        check_for_unsupported_schema(obj, tool_name)?;
    }

    // 1. Enum
    if let Some(enum_arr) = schema.get("enum").and_then(Value::as_array) {
        if !enum_arr.is_empty() {
            let vals: Vec<&str> = enum_arr.iter().filter_map(Value::as_str).collect();
            if !vals.is_empty() {
                return Ok(vals.join("|"));
            }
        }
    }

    // 2. Type
    if let Some(type_val) = schema.get("type").and_then(Value::as_str) {
        match type_val {
            "string" => {
                if let Some(format_str) = schema.get("format").and_then(Value::as_str) {
                    match format_str {
                        "date-time" | "datetime" => Ok("datetime".to_string()),
                        "date" => Ok("date".to_string()),
                        "time" => Ok("time".to_string()),
                        "email" => Ok("email".to_string()),
                        "uri" => Ok("uri".to_string()),
                        _ => Ok("str".to_string()),
                    }
                } else {
                    Ok("str".to_string())
                }
            }
            "integer" => Ok("int".to_string()),
            "number" => Ok("float".to_string()),
            "boolean" => Ok("bool".to_string()),
            "array" => {
                if let Some(items_schema) = schema.get("items") {
                    let inner = encode_type_schema(items_schema, tool_name)?;
                    Ok(format!("[{inner}]"))
                } else {
                    Ok("[any]".to_string())
                }
            }
            "object" => Ok("object".to_string()),
            "null" => Ok("null".to_string()),
            other => Ok(other.to_string()),
        }
    } else {
        Ok("any".to_string())
    }
}
