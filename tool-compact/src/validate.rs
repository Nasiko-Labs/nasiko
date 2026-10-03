use serde_json::Value;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    #[error("expected JSON object for tool arguments, got {0}")]
    NotAnObject(String),
    #[error("missing required parameter '{0}'")]
    MissingRequired(String),
    #[error("parameter '{name}' expected type '{expected}', got {actual}")]
    TypeMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    #[error("parameter '{name}' value '{actual}' is not in allowed enum: {allowed:?}")]
    InvalidEnum {
        name: String,
        allowed: Vec<String>,
        actual: String,
    },
    #[error("array item in parameter '{name}' expected type '{expected}'")]
    InvalidArrayItem {
        name: String,
        expected: String,
    },
}

pub fn validate_arguments(
    args: &Value,
    parameters_schema: Option<&Value>,
) -> Result<(), ValidationError> {
    let Some(schema) = parameters_schema else {
        return Ok(());
    };

    let obj = match args {
        Value::Object(map) => map,
        _ => return Err(ValidationError::NotAnObject(format!("{:?}", args))),
    };

    // Check required fields
    if let Some(Value::Array(req_fields)) = schema.get("required") {
        for req in req_fields {
            if let Some(req_name) = req.as_str() {
                match obj.get(req_name) {
                    None | Some(Value::Null) => {
                        return Err(ValidationError::MissingRequired(req_name.to_string()));
                    }
                    _ => {}
                }
            }
        }
    }

    // Check properties
    if let Some(Value::Object(properties)) = schema.get("properties") {
        for (prop_name, prop_val) in obj {
            if let Some(prop_schema) = properties.get(prop_name) {
                validate_property_value(prop_name, prop_val, prop_schema)?;
            }
        }
    }

    Ok(())
}

fn validate_property_value(
    prop_name: &str,
    val: &Value,
    schema: &Value,
) -> Result<(), ValidationError> {
    if val.is_null() {
        return Ok(());
    }

    // Check enum
    if let Some(Value::Array(enum_vals)) = schema.get("enum") {
        let mut allowed = Vec::new();
        let mut matched = false;
        for ev in enum_vals {
            if let Some(s) = ev.as_str() {
                allowed.push(s.to_string());
                if val.as_str() == Some(s) {
                    matched = true;
                }
            } else if ev == val {
                matched = true;
            }
        }
        if !matched {
            return Err(ValidationError::InvalidEnum {
                name: prop_name.to_string(),
                allowed,
                actual: val.to_string(),
            });
        }
    }

    // Check type
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str) {
        match expected_type {
            "string" => {
                if !val.is_string() {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "string".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            }
            "integer" => {
                if !val.is_i64() && !val.is_u64() {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "integer".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            }
            "number" => {
                if !val.is_number() {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "number".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            }
            "boolean" => {
                if !val.is_boolean() {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "boolean".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            }
            "array" => {
                if let Value::Array(items) = val {
                    if let Some(item_schema) = schema.get("items")
                        && let Some(item_type) = item_schema.get("type").and_then(Value::as_str) {
                            for item in items {
                                match item_type {
                                    "string" if !item.is_string() => {
                                        return Err(ValidationError::InvalidArrayItem {
                                            name: prop_name.to_string(),
                                            expected: "string".to_string(),
                                        });
                                    }
                                    "integer" if (!item.is_i64() && !item.is_u64()) => {
                                        return Err(ValidationError::InvalidArrayItem {
                                            name: prop_name.to_string(),
                                            expected: "integer".to_string(),
                                        });
                                    }
                                    "number" if !item.is_number() => {
                                        return Err(ValidationError::InvalidArrayItem {
                                            name: prop_name.to_string(),
                                            expected: "number".to_string(),
                                        });
                                    }
                                    _ => {}
                                }
                            }
                        }
                } else {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "array".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            }
            "object"
                if !val.is_object() => {
                    return Err(ValidationError::TypeMismatch {
                        name: prop_name.to_string(),
                        expected: "object".to_string(),
                        actual: value_type_name(val).to_string(),
                    });
                }
            _ => {}
        }
    }

    Ok(())
}

fn value_type_name(val: &Value) -> &'static str {
    match val {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
