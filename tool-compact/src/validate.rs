use serde_json::Value;

use crate::error::ToolCompactError;
use crate::types::ToolDef;

/// Validate a tool call against the registered tools and their JSON schemas.
pub fn validate_tool_call(
    tool_name: &str,
    arguments_str: &str,
    tools: &[ToolDef],
) -> Result<(), ToolCompactError> {
    // 1. Look up tool
    let tool = tools
        .iter()
        .find(|t| t.function.name == tool_name)
        .ok_or_else(|| ToolCompactError::UnknownTool(tool_name.to_string()))?;

    // 2. Parse arguments JSON
    let args_val: Value = serde_json::from_str(arguments_str).map_err(|e| {
        ToolCompactError::InvalidArguments(format!("Malformed JSON arguments: {e}"))
    })?;

    let args_obj = args_val.as_object().ok_or_else(|| {
        ToolCompactError::InvalidArguments("Arguments must be a JSON object".to_string())
    })?;

    // 3. Validate against parameters schema if present
    if let Some(ref params) = tool.function.parameters {
        if let Some(params_obj) = params.as_object() {
            // 3a. Check required fields
            if let Some(req_arr) = params_obj.get("required").and_then(Value::as_array) {
                for req in req_arr {
                    if let Some(field_name) = req.as_str() {
                        match args_obj.get(field_name) {
                            None => {
                                return Err(ToolCompactError::InvalidArguments(format!(
                                    "Missing required field: '{field_name}'"
                                )));
                            }
                            Some(Value::Null) => {
                                let allows_null = params_obj
                                    .get("properties")
                                    .and_then(Value::as_object)
                                    .and_then(|p| p.get(field_name))
                                    .is_some_and(|prop_schema| {
                                        if prop_schema.get("nullable") == Some(&Value::Bool(true)) {
                                            return true;
                                        }
                                        if let Some(types) = prop_schema.get("type").and_then(Value::as_array) {
                                            return types.iter().any(|t| t == "null");
                                        }
                                        prop_schema.get("type") == Some(&Value::String("null".to_string()))
                                    });

                                if !allows_null {
                                    return Err(ToolCompactError::InvalidArguments(format!(
                                        "Required field '{field_name}' cannot be null"
                                    )));
                                }
                            }
                            Some(_) => {}
                        }
                    }
                }
            }

            // 3b. Check property types and enums
            if let Some(props_obj) = params_obj.get("properties").and_then(Value::as_object) {
                for (key, val) in args_obj {
                    if let Some(prop_schema) = props_obj.get(key) {
                        validate_value(key, val, prop_schema)?;
                    } else if params_obj.get("additionalProperties") == Some(&Value::Bool(false)) {
                        return Err(ToolCompactError::InvalidArguments(format!(
                            "Unknown property: '{key}'"
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_value(key: &str, val: &Value, schema: &Value) -> Result<(), ToolCompactError> {
    // Nullable handling
    if val.is_null() {
        if schema.get("nullable") == Some(&Value::Bool(true)) {
            return Ok(());
        }
        if let Some(types) = schema.get("type").and_then(Value::as_array) {
            if types.iter().any(|t| t == "null") {
                return Ok(());
            }
        }
        if schema.get("type") == Some(&Value::String("null".to_string())) {
            return Ok(());
        }
    }

    // Enum validation
    if let Some(enum_arr) = schema.get("enum").and_then(Value::as_array) {
        if !enum_arr.contains(val) {
            let allowed: Vec<String> = enum_arr.iter().map(|v| v.to_string()).collect();
            return Err(ToolCompactError::InvalidArguments(format!(
                "Field '{key}' has value {val}, expected one of: [{}]",
                allowed.join(", ")
            )));
        }
    }

    // Type validation
    if let Some(type_val) = schema.get("type") {
        if let Some(type_str) = type_val.as_str() {
            match type_str {
                "string" => {
                    if !val.is_string() {
                        return Err(ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected string, got {val}"
                        )));
                    }
                }
                "integer" => {
                    if !val.is_i64() && !val.is_u64() {
                        return Err(ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected integer, got {val}"
                        )));
                    }
                }
                "number" => {
                    if !val.is_number() {
                        return Err(ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected number, got {val}"
                        )));
                    }
                }
                "boolean" => {
                    if !val.is_boolean() {
                        return Err(ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected boolean, got {val}"
                        )));
                    }
                }
                "array" => {
                    let arr = val.as_array().ok_or_else(|| {
                        ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected array, got {val}"
                        ))
                    })?;
                    if let Some(items_schema) = schema.get("items") {
                        for (idx, item) in arr.iter().enumerate() {
                            let item_key = format!("{key}[{idx}]");
                            validate_value(&item_key, item, items_schema)?;
                        }
                    }
                }
                "object" => {
                    let obj = val.as_object().ok_or_else(|| {
                        ToolCompactError::InvalidArguments(format!(
                            "Field '{key}' expected object, got {val}"
                        ))
                    })?;
                    if let Some(nested_props) = schema.get("properties").and_then(Value::as_object)
                    {
                        if let Some(nested_req) = schema.get("required").and_then(Value::as_array) {
                            for req_f in nested_req.iter().filter_map(Value::as_str) {
                                match obj.get(req_f) {
                                    None | Some(Value::Null) => {
                                        return Err(ToolCompactError::InvalidArguments(format!(
                                            "Missing required nested field '{key}.{req_f}'"
                                        )));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        for (sub_k, sub_v) in obj {
                            if let Some(sub_schema) = nested_props.get(sub_k) {
                                let sub_key = format!("{key}.{sub_k}");
                                validate_value(&sub_key, sub_v, sub_schema)?;
                            }
                        }
                    }
                }
                "null" if !val.is_null() => {
                    return Err(ToolCompactError::InvalidArguments(format!(
                        "Field '{key}' expected null, got {val}"
                    )));
                }
                _ => {}
            }
        } else if let Some(type_arr) = type_val.as_array() {
            let mut matches_any = false;
            for t in type_arr.iter().filter_map(Value::as_str) {
                let matches = match t {
                    "string" => val.is_string(),
                    "integer" => val.is_i64() || val.is_u64(),
                    "number" => val.is_number(),
                    "boolean" => val.is_boolean(),
                    "array" => val.is_array(),
                    "object" => val.is_object(),
                    "null" => val.is_null(),
                    _ => false,
                };
                if matches {
                    matches_any = true;
                    break;
                }
            }
            if !matches_any {
                return Err(ToolCompactError::InvalidArguments(format!(
                    "Field '{key}' value {val} does not match any allowed type in {type_arr:?}"
                )));
            }
        }
    }

    Ok(())
}
