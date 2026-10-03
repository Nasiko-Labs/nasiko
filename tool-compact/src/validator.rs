//! JSON Schema support checks and fail-closed argument validation.

use serde_json::{Map, Value};

use crate::error::CompactError;

/// Keywords that mean "bypass compaction" — we cannot preserve meaning compactly.
const UNSUPPORTED_KEYS: &[&str] = &[
    "$ref",
    "$defs",
    "definitions",
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "dependentRequired",
    "dependentSchemas",
    "patternProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "prefixItems",
    "contains",
    "contentMediaType",
    "contentEncoding",
];

/// Return `Ok(())` if `parameters` is supported, else [`CompactError::UnsupportedSchema`].
pub fn check_supported(parameters: Option<&Value>) -> Result<(), CompactError> {
    let Some(schema) = parameters else {
        return Ok(());
    };
    check_node(schema, 0)
}

fn check_node(schema: &Value, depth: usize) -> Result<(), CompactError> {
    if depth > 8 {
        return Err(CompactError::UnsupportedSchema(
            "schema nesting deeper than 8".into(),
        ));
    }
    let Value::Object(map) = schema else {
        return Err(CompactError::UnsupportedSchema(
            "schema must be an object".into(),
        ));
    };
    for key in UNSUPPORTED_KEYS {
        if map.contains_key(*key) {
            return Err(CompactError::UnsupportedSchema((*key).into()));
        }
    }
    // `additionalProperties` as a nested schema is unsupported; `false`/`true` are fine.
    if let Some(ap) = map.get("additionalProperties")
        && ap.is_object()
    {
        return Err(CompactError::UnsupportedSchema(
            "additionalProperties as schema".into(),
        ));
    }
    // `items` as a tuple (array) is unsupported; single schema is fine.
    if let Some(Value::Array(_)) = map.get("items") {
        return Err(CompactError::UnsupportedSchema("tuple items arrays".into()));
    }

    if let Some(props) = map.get("properties").and_then(Value::as_object) {
        for prop_schema in props.values() {
            check_node(prop_schema, depth + 1)?;
        }
    }
    if let Some(items) = map.get("items") {
        check_node(items, depth + 1)?;
    }
    Ok(())
}

/// Validate `args` against the tool's JSON Schema. Fail closed — never coerce.
pub fn validate_args(parameters: Option<&Value>, args: &Value) -> Result<(), CompactError> {
    let Value::Object(obj) = args else {
        return Err(CompactError::InvalidArguments);
    };
    let Some(schema) = parameters else {
        // No schema ⇒ only empty object is accepted.
        return if obj.is_empty() {
            Ok(())
        } else {
            Err(CompactError::InvalidArguments)
        };
    };
    validate_object(schema, obj)
}

fn validate_object(schema: &Value, obj: &Map<String, Value>) -> Result<(), CompactError> {
    let Value::Object(schema_map) = schema else {
        return Err(CompactError::InvalidArguments);
    };

    let required: Vec<&str> = schema_map
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for key in &required {
        if !obj.contains_key(*key) {
            return Err(CompactError::InvalidArguments);
        }
    }

    let props = schema_map
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let additional_allowed = !matches!(
        schema_map.get("additionalProperties"),
        Some(Value::Bool(false))
    );

    for (key, value) in obj {
        match props.get(key) {
            Some(prop_schema) => validate_value(prop_schema, value)?,
            None if additional_allowed => {}
            None => return Err(CompactError::InvalidArguments),
        }
    }
    Ok(())
}

fn validate_value(schema: &Value, value: &Value) -> Result<(), CompactError> {
    let Value::Object(schema_map) = schema else {
        return Err(CompactError::InvalidArguments);
    };

    if let Some(enum_vals) = schema_map.get("enum").and_then(Value::as_array) {
        if !enum_vals.iter().any(|e| e == value) {
            return Err(CompactError::InvalidArguments);
        }
        // Enum already constrains the value; type check is still applied when present.
    }

    if let Some(ty) = schema_map.get("type") {
        match ty {
            Value::String(t) => check_type(t, value)?,
            Value::Array(types) => {
                let ok = types
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|t| check_type(t, value).is_ok());
                if !ok {
                    return Err(CompactError::InvalidArguments);
                }
            }
            _ => return Err(CompactError::InvalidArguments),
        }
    }

    if let Some(props) = schema_map.get("properties")
        && value.is_object()
    {
        let Value::Object(obj) = value else {
            return Err(CompactError::InvalidArguments);
        };
        // Recurse with a synthetic object schema that keeps required/additional.
        let mut nested = schema_map.clone();
        nested.insert("properties".into(), props.clone());
        return validate_object(&Value::Object(nested), obj);
    }

    if let Some(items) = schema_map.get("items") {
        let Value::Array(arr) = value else {
            // type check above should have caught this when type is array
            if schema_map.get("type").and_then(Value::as_str) == Some("array") {
                return Err(CompactError::InvalidArguments);
            }
            return Ok(());
        };
        for item in arr {
            validate_value(items, item)?;
        }
    }

    Ok(())
}

fn check_type(ty: &str, value: &Value) -> Result<(), CompactError> {
    let ok = match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.as_f64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(CompactError::InvalidArguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_any_of() {
        let err = check_supported(Some(&json!({"anyOf": []}))).unwrap_err();
        assert!(matches!(err, CompactError::UnsupportedSchema(_)));
    }

    #[test]
    fn missing_required_fails() {
        let schema = json!({
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"]
        });
        assert!(validate_args(Some(&schema), &json!({})).is_err());
    }

    #[test]
    fn bad_enum_fails() {
        let schema = json!({
            "type": "object",
            "properties": {
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": []
        });
        assert!(validate_args(Some(&schema), &json!({"visibility": "secret"})).is_err());
    }
}
