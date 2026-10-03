use serde_json::Value;
use std::collections::HashSet;

use crate::error::CompactError;
use crate::types::{CompactTools, ToolDef};

pub const INSTRUCTION_PROMPT: &str = "To call a tool, emit: <<call name {json args}>>";

/// Compact a list of tool definitions into the DSL grammar and call instruction.
/// Returns CompactError if any tool uses unsupported JSON Schema features or is malformed.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Err(CompactError::EmptyTools);
    }

    let mut schema_lines = Vec::with_capacity(tools.len());
    let mut tool_names = Vec::with_capacity(tools.len());

    for tool in tools {
        let fn_def = &tool.function;
        let name = fn_def.name.trim();
        if name.is_empty() {
            return Err(CompactError::MissingFunctionName);
        }
        tool_names.push(name.to_string());

        let params_dsl = if let Some(params) = &fn_def.parameters {
            check_unsupported(params)?;
            encode_parameters(params)?
        } else {
            String::new()
        };

        let mut line = format!("{name}({params_dsl})");
        if let Some(desc) = &fn_def.description {
            let clean_desc = desc.replace('\n', " ").trim().to_string();
            if !clean_desc.is_empty() {
                line.push_str(" - ");
                line.push_str(&clean_desc);
            }
        }
        schema_lines.push(line);
    }

    let schema_dsl = schema_lines.join("\n");
    let instruction = INSTRUCTION_PROMPT.to_string();
    let prompt = format!("{schema_dsl}\n{instruction}");

    Ok(CompactTools {
        prompt,
        schema_dsl,
        instruction,
        tool_names,
    })
}

fn check_unsupported(val: &Value) -> Result<(), CompactError> {
    if let Some(obj) = val.as_object() {
        for key in ["anyOf", "oneOf", "allOf", "$ref", "patternProperties"] {
            if obj.contains_key(key) {
                return Err(CompactError::UnsupportedSchema(key.to_string()));
            }
        }
        for sub in obj.values() {
            check_unsupported(sub)?;
        }
    } else if let Some(arr) = val.as_array() {
        for item in arr {
            check_unsupported(item)?;
        }
    }
    Ok(())
}

fn encode_parameters(schema: &Value) -> Result<String, CompactError> {
    let Some(obj) = schema.as_object() else {
        return Ok(String::new());
    };

    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        return Ok(String::new());
    };

    let req_list: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let req_set: HashSet<&str> = req_list.iter().copied().collect();

    let mut param_parts = Vec::new();

    // 1. Required parameters in declared required order
    for req_field in &req_list {
        if let Some(prop_schema) = props.get(*req_field) {
            let ty_str = format_param_type(prop_schema);
            param_parts.push(format!("{req_field}:{ty_str}"));
        }
    }

    // 2. Optional parameters (sorted deterministically)
    let mut optional_keys: Vec<&String> = props
        .keys()
        .filter(|k| !req_set.contains(k.as_str()))
        .collect();
    optional_keys.sort();

    for opt_field in optional_keys {
        if let Some(prop_schema) = props.get(opt_field) {
            let ty_str = format_param_type(prop_schema);
            param_parts.push(format!("{opt_field}?:{ty_str}"));
        }
    }

    Ok(param_parts.join(", "))
}

fn format_param_type(schema: &Value) -> String {
    // 1. Enum
    if let Some(enums) = schema.get("enum").and_then(Value::as_array) {
        let enum_strs: Vec<&str> = enums.iter().filter_map(Value::as_str).collect();
        if !enum_strs.is_empty() {
            return enum_strs.join("|");
        }
    }

    // 2. Format
    if let Some(format) = schema.get("format").and_then(Value::as_str) {
        match format {
            "date-time" => return "datetime".to_string(),
            "email" => return "email".to_string(),
            "uuid" => return "uuid".to_string(),
            _ => {}
        }
    }

    // 3. Type
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "str".to_string(),
        Some("integer") => "int".to_string(),
        Some("number") => "num".to_string(),
        Some("boolean") => "bool".to_string(),
        Some("array") => {
            if let Some(items) = schema.get("items") {
                let inner = format_param_type(items);
                format!("[{inner}]")
            } else {
                "[any]".to_string()
            }
        }
        Some("object") => {
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                let sub_req_set: HashSet<&str> = schema
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let mut parts = Vec::new();
                for (k, v) in props {
                    let opt = if sub_req_set.contains(k.as_str()) {
                        ""
                    } else {
                        "?"
                    };
                    let ty = format_param_type(v);
                    parts.push(format!("{k}{opt}:{ty}"));
                }
                format!("{{{}}}", parts.join(", "))
            } else {
                "object".to_string()
            }
        }
        _ => "any".to_string(),
    }
}
