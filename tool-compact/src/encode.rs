use std::collections::HashSet;
use serde_json::{Map, Value};

use crate::types::{CompactTools, FunctionDef, ToolDef};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EncodeError {
    #[error("failed to encode tool '{0}': {1}")]
    ToolEncodingFailed(String, String),
}

/// Encode standard tool definitions into the ultra-compact format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut signatures = Vec::with_capacity(tools.len());
    let mut tool_names = Vec::with_capacity(tools.len());

    for tool in tools {
        let name = &tool.function.name;
        tool_names.push(name.clone());

        let mut sig = format_tool_signature(tool)?;
        if let Some(desc) = &tool.function.description {
            sig.push_str(" - ");
            sig.push_str(desc.trim());
        }

        signatures.push(sig);
    }

    let instructions = "To call a tool, emit: <<call name {json args}>>".to_string();

    let mut prompt = String::new();
    for sig in &signatures {
        prompt.push_str(sig);
        prompt.push('\n');
    }
    prompt.push_str(&instructions);

    Ok(CompactTools {
        prompt,
        signatures,
        instructions,
        tool_names,
    })
}

fn format_tool_signature(tool: &ToolDef) -> Result<String, EncodeError> {
    let name = &tool.function.name;
    let mut out = format!("{}(", name);

    if let Some(params) = &tool.function.parameters {
        let required_set: HashSet<String> = params
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        if let Some(Value::Object(props)) = params.get("properties") {
            let mut first = true;
            for (prop_name, prop_schema) in props {
                if !first {
                    out.push_str(", ");
                }
                first = false;

                let is_req = required_set.contains(prop_name);
                out.push_str(prop_name);
                if !is_req {
                    out.push('?');
                }
                out.push(':');
                out.push_str(&format_prop_type(prop_schema));
            }
        }
    }

    out.push(')');
    Ok(out)
}

fn format_prop_type(schema: &Value) -> String {
    // Enum
    if let Some(Value::Array(enums)) = schema.get("enum") {
        let vals: Vec<String> = enums
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                _ => v.to_string(),
            })
            .collect();
        return vals.join("|");
    }

    if let Some(fmt) = schema.get("format").and_then(Value::as_str) {
        if fmt == "date-time" || fmt == "datetime" {
            return "datetime".to_string();
        }
    }

    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("any");
    match ty {
        "string" => "str".to_string(),
        "integer" => "int".to_string(),
        "number" => "num".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            if let Some(items) = schema.get("items") {
                format!("[{}]", format_prop_type(items))
            } else {
                "[any]".to_string()
            }
        }
        "object" => "obj".to_string(),
        other => other.to_string(),
    }
}

/// Decode a CompactTools representation back into standard ToolDefs.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, EncodeError> {
    let mut tools = Vec::new();

    for sig in &compact.signatures {
        let line = sig.trim();
        if line.is_empty() {
            continue;
        }

        // Parse `name(params) - description`
        let open_paren = line.find('(').ok_or_else(|| {
            EncodeError::ToolEncodingFailed(line.to_string(), "missing '('".to_string())
        })?;
        let name = line[..open_paren].trim().to_string();

        let close_paren = line.rfind(')').ok_or_else(|| {
            EncodeError::ToolEncodingFailed(line.to_string(), "missing ')'".to_string())
        })?;

        let params_part = &line[open_paren + 1..close_paren];
        let description = if let Some(dash_idx) = line[close_paren + 1..].find('-') {
            Some(line[close_paren + 1 + dash_idx + 1..].trim().to_string())
        } else {
            None
        };

        let mut properties = Map::new();
        let mut required = Vec::new();

        if !params_part.trim().is_empty() {
            for param_str in split_params(params_part) {
                let param_str = param_str.trim();
                if param_str.is_empty() {
                    continue;
                }

                let colon_idx = match param_str.find(':') {
                    Some(idx) => idx,
                    None => continue,
                };

                let name_part = param_str[..colon_idx].trim();
                let type_part = param_str[colon_idx + 1..].trim();

                let (pname, is_req) = if let Some(stripped) = name_part.strip_suffix('?') {
                    (stripped.trim(), false)
                } else {
                    (name_part, true)
                };

                if is_req {
                    required.push(Value::String(pname.to_string()));
                }

                let prop_schema = parse_prop_type_string(type_part);
                properties.insert(pname.to_string(), Value::Object(prop_schema));
            }
        }

        let mut parameters_obj = Map::new();
        parameters_obj.insert("type".to_string(), Value::String("object".to_string()));
        parameters_obj.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            parameters_obj.insert("required".to_string(), Value::Array(required));
        }

        tools.push(ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name,
                description,
                parameters: Some(Value::Object(parameters_obj)),
            },
            extra: Map::new(),
        });
    }

    Ok(tools)
}

fn split_params(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for ch in s.chars() {
        match ch {
            '[' | '(' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ']' | ')' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

fn parse_prop_type_string(s: &str) -> Map<String, Value> {
    let mut map = Map::new();
    let trimmed = s.trim();

    // Check enum
    if trimmed.contains('|') {
        let vals: Vec<Value> = trimmed
            .split('|')
            .map(|part| {
                let p = part.trim().trim_matches('"');
                Value::String(p.to_string())
            })
            .collect();
        map.insert("type".to_string(), Value::String("string".to_string()));
        map.insert("enum".to_string(), Value::Array(vals));
        return map;
    }

    // Check array
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1].trim();
        let inner_schema = parse_prop_type_string(inner);
        map.insert("type".to_string(), Value::String("array".to_string()));
        map.insert("items".to_string(), Value::Object(inner_schema));
        return map;
    }

    match trimmed {
        "datetime" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        "str" | "string" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
        }
        "int" | "integer" => {
            map.insert("type".to_string(), Value::String("integer".to_string()));
        }
        "num" | "number" => {
            map.insert("type".to_string(), Value::String("number".to_string()));
        }
        "bool" | "boolean" => {
            map.insert("type".to_string(), Value::String("boolean".to_string()));
        }
        "obj" | "object" => {
            map.insert("type".to_string(), Value::String("object".to_string()));
        }
        _ => {
            map.insert("type".to_string(), Value::String(trimmed.to_string()));
        }
    }

    map
}
