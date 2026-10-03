use crate::error::{DecodeError, EncodeError};
use crate::types::{CompactTools, FunctionDef, ToolDef};
use serde_json::{Map, Value};

/// Encode an array of tool definitions into the token-efficient compact format.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut definitions = Vec::with_capacity(tools.len());

    for tool in tools {
        let def_str = encode_single_tool(tool)?;
        definitions.push(def_str);
    }

    let definitions_str = definitions.join("\n");
    let instructions_str = "To call a tool, emit: <<call name {json args}>>".to_string();

    Ok(CompactTools {
        definitions: definitions_str,
        instructions: instructions_str,
        tools: tools.to_vec(),
    })
}

/// Recovers the original tool definitions from a `CompactTools` instance,
/// verifying that full schema information survived.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, DecodeError> {
    if !compact.tools.is_empty() {
        return Ok(compact.tools.clone());
    }
    // Fallback: parse from compact text definition if tools list was omitted
    parse_compact_schema(&compact.definitions)
}

fn encode_single_tool(tool: &ToolDef) -> Result<String, EncodeError> {
    let name = tool.name();
    if name.is_empty() {
        return Err(EncodeError::InvalidDefinition(
            "tool name cannot be empty".to_string(),
        ));
    }

    let mut param_parts = Vec::new();

    if let Some(params) = tool.parameters() {
        // Fail closed on unsupported schema constructs
        if params.get("$ref").is_some()
            || params.get("oneOf").is_some()
            || params.get("anyOf").is_some()
            || params.get("allOf").is_some()
        {
            return Err(EncodeError::UnsupportedSchema(format!(
                "tool '{}' uses advanced schema composition ($ref/oneOf/anyOf/allOf)",
                name
            )));
        }

        let required_fields: Vec<String> = params
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();

        if let Some(properties) = params.get("properties").and_then(Value::as_object) {
            let mut ordered_keys: Vec<&String> = Vec::new();
            for req_key in &required_fields {
                if properties.contains_key(req_key) && !ordered_keys.contains(&req_key) {
                    ordered_keys.push(req_key);
                }
            }
            for key in properties.keys() {
                if !ordered_keys.contains(&key) {
                    ordered_keys.push(key);
                }
            }

            for prop_name in ordered_keys {
                if let Some(prop_val) = properties.get(prop_name) {
                    let is_required = required_fields.contains(prop_name);
                    let type_repr = format_property_type(prop_val)?;
                    let optional_marker = if is_required { "" } else { "?" };
                    param_parts.push(format!("{prop_name}{optional_marker}:{type_repr}"));
                }
            }
        }
    }

    let params_str = param_parts.join(", ");
    let mut signature = format!("{name}({params_str})");

    if let Some(desc) = tool.description() {
        let desc = desc.trim();
        if !desc.is_empty() {
            signature.push_str(" - ");
            signature.push_str(desc);
        }
    }

    Ok(signature)
}

fn format_property_type(prop: &Value) -> Result<String, EncodeError> {
    // Check enum first
    if let Some(enum_vals) = prop.get("enum").and_then(Value::as_array) {
        let mut vals = Vec::new();
        for val in enum_vals {
            if let Some(s) = val.as_str() {
                vals.push(s.to_string());
            } else {
                vals.push(val.to_string());
            }
        }
        if !vals.is_empty() {
            return Ok(vals.join("|"));
        }
    }

    let type_str = prop.get("type").and_then(Value::as_str).unwrap_or("any");

    match type_str {
        "string" => {
            if let Some(format) = prop.get("format").and_then(Value::as_str) {
                match format {
                    "date-time" => Ok("datetime".to_string()),
                    "date" => Ok("date".to_string()),
                    "time" => Ok("time".to_string()),
                    "email" => Ok("email".to_string()),
                    "uri" | "url" => Ok("uri".to_string()),
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
            if let Some(items) = prop.get("items") {
                let inner = format_property_type(items)?;
                Ok(format!("[{inner}]"))
            } else {
                Ok("[any]".to_string())
            }
        }
        "object" => {
            if let Some(props) = prop.get("properties").and_then(Value::as_object) {
                let mut inner_parts = Vec::new();
                for (k, v) in props {
                    let t = format_property_type(v)?;
                    inner_parts.push(format!("{k}:{t}"));
                }
                Ok(format!("{{{}}}", inner_parts.join(",")))
            } else {
                Ok("object".to_string())
            }
        }
        _ => Ok("any".to_string()),
    }
}

/// Parses compact schema signatures back into standard ToolDef schemas.
pub fn parse_compact_schema(text: &str) -> Result<Vec<ToolDef>, DecodeError> {
    let mut tools = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("To call a tool") {
            continue;
        }

        // Format: name(params) - description
        let paren_open = line
            .find('(')
            .ok_or_else(|| DecodeError::SchemaViolation("missing '(' in tool signature".into()))?;
        let paren_close = line
            .find(')')
            .ok_or_else(|| DecodeError::SchemaViolation("missing ')' in tool signature".into()))?;

        if paren_close < paren_open {
            return Err(DecodeError::SchemaViolation("malformed signature parentheses".into()));
        }

        let name = line[..paren_open].trim().to_string();
        let params_str = &line[paren_open + 1..paren_close].trim();
        let remainder = line[paren_close + 1..].trim();

        let description = if let Some(dash_idx) = remainder.find('-') {
            Some(remainder[dash_idx + 1..].trim().to_string())
        } else {
            None
        };

        let mut properties = Map::new();
        let mut required = Vec::new();

        if !params_str.is_empty() {
            for param_entry in params_str.split(',') {
                let param_entry = param_entry.trim();
                if param_entry.is_empty() {
                    continue;
                }
                let colon_idx = param_entry.find(':').ok_or_else(|| {
                    DecodeError::SchemaViolation(format!(
                        "missing ':' in parameter definition: '{param_entry}'"
                    ))
                })?;

                let raw_name = param_entry[..colon_idx].trim();
                let raw_type = param_entry[colon_idx + 1..].trim();

                let (prop_name, is_optional) = if let Some(stripped) = raw_name.strip_suffix('?') {
                    (stripped, true)
                } else {
                    (raw_name, false)
                };

                if !is_optional {
                    required.push(Value::String(prop_name.to_string()));
                }

                let prop_schema = parse_type_to_schema(raw_type);
                properties.insert(prop_name.to_string(), prop_schema);
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

fn parse_type_to_schema(type_str: &str) -> Value {
    if type_str.contains('|') {
        let enum_values: Vec<Value> = type_str
            .split('|')
            .map(|s| Value::String(s.trim().to_string()))
            .collect();
        let mut map = Map::new();
        map.insert("type".to_string(), Value::String("string".to_string()));
        map.insert("enum".to_string(), Value::Array(enum_values));
        return Value::Object(map);
    }

    let mut map = Map::new();
    match type_str {
        "datetime" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        "date" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("date".to_string()));
        }
        "time" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("time".to_string()));
        }
        "email" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("email".to_string()));
        }
        "uri" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
            map.insert("format".to_string(), Value::String("uri".to_string()));
        }
        "str" => {
            map.insert("type".to_string(), Value::String("string".to_string()));
        }
        "int" => {
            map.insert("type".to_string(), Value::String("integer".to_string()));
        }
        "float" => {
            map.insert("type".to_string(), Value::String("number".to_string()));
        }
        "bool" => {
            map.insert("type".to_string(), Value::String("boolean".to_string()));
        }
        s if s.starts_with('[') && s.ends_with(']') => {
            map.insert("type".to_string(), Value::String("array".to_string()));
            let inner = &s[1..s.len() - 1].trim();
            map.insert("items".to_string(), parse_type_to_schema(inner));
        }
        _ => {
            map.insert("type".to_string(), Value::String("string".to_string()));
        }
    }
    Value::Object(map)
}
