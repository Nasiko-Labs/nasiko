use crate::error::ToolCompactError;
use crate::types::{CompactTools, ToolDef};
use serde_json::Value;

/// Format a JSON Schema type into a compact notation.
fn format_schema_type(prop: &Value) -> String {
    if let Some(enum_vals) = prop.get("enum").and_then(Value::as_array) {
        let items: Vec<String> = enum_vals
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if !items.is_empty() {
            return items.join("|");
        }
    }

    let ty = prop.get("type").and_then(Value::as_str).unwrap_or("any");
    match ty {
        "string" => {
            if prop.get("format").and_then(Value::as_str) == Some("date-time") {
                "datetime".to_string()
            } else {
                "str".to_string()
            }
        }
        "integer" => "int".to_string(),
        "number" => "float".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            let item_type = prop
                .get("items")
                .map(format_schema_type)
                .unwrap_or_else(|| "any".to_string());
            format!("[{}]", item_type)
        }
        "object" => "{json}".to_string(),
        other => other.to_string(),
    }
}

/// Encode a single ToolDef into its compact signature line.
pub fn encode_tool_signature(tool: &ToolDef) -> String {
    let name = &tool.function.name;
    let mut params_str = String::new();

    if let Some(params) = &tool.function.parameters {
        let required_fields: Vec<String> = params
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        if let Some(properties) = params.get("properties").and_then(Value::as_object) {
            let mut parts = Vec::new();
            for (prop_name, prop_val) in properties {
                let is_req = required_fields.contains(prop_name);
                let ty_str = format_schema_type(prop_val);
                if is_req {
                    parts.push(format!("{}:{}", prop_name, ty_str));
                } else {
                    parts.push(format!("{}?:{}", prop_name, ty_str));
                }
            }
            params_str = parts.join(", ");
        }
    }

    let desc_part = match &tool.function.description {
        Some(desc) if !desc.trim().is_empty() => format!(" - {}", desc.trim()),
        _ => String::new(),
    };

    format!("{}({}){}", name, params_str, desc_part)
}

/// Encode a slice of ToolDefs into CompactTools.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, ToolCompactError> {
    let mut sig_lines = Vec::new();
    for tool in tools {
        sig_lines.push(encode_tool_signature(tool));
    }

    let signatures = sig_lines.join("\n");
    let instructions = concat!(
        "To call a tool, emit: <<call tool_name {json args}>>\n",
        "Escape '>>' inside string arguments as '\\>>'.\n",
        "If you do not need to call a tool, respond with plain text without any <<call ...>> syntax."
    )
    .to_string();

    let prompt_section = if tools.is_empty() {
        String::new()
    } else {
        format!(
            "Available tools:\n{}\n\nCall instructions:\n{}",
            signatures, instructions
        )
    };

    Ok(CompactTools {
        signatures,
        instructions,
        prompt_section,
        original_tools: tools.to_vec(),
    })
}

/// Reconstruct the tool definitions from CompactTools (preserves full schemas).
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, ToolCompactError> {
    Ok(compact.original_tools.clone())
}
