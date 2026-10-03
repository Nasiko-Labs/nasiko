use serde_json::Value;

/// The canonical list of supported JSON Schema keywords for compact tools.
/// Anything outside this set will cause `is_schema_supported` to return an `Err`.
pub const SUPPORTED_KEYWORDS: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "description",
    "format",
];

pub const SUPPORTED_PRIMITIVE_TYPES: &[&str] =
    &["string", "integer", "number", "boolean", "array", "object"];

/// Validates whether a JSON schema is supported by the compact tools encoder.
/// Returns `Ok(())` if fully supported, or `Err(reason)` naming the unsupported keyword/feature.
pub fn is_schema_supported(schema: &Value) -> Result<(), String> {
    match schema {
        Value::Object(map) => {
            // Check for unsupported keywords
            for key in map.keys() {
                if !SUPPORTED_KEYWORDS.contains(&key.as_str()) {
                    return Err(format!("unsupported keyword '{key}'"));
                }
            }

            // Check "type"
            if let Some(type_val) = map.get("type") {
                match type_val {
                    Value::String(t) => {
                        if !SUPPORTED_PRIMITIVE_TYPES.contains(&t.as_str()) {
                            return Err(format!("unsupported type '{t}'"));
                        }
                    }
                    Value::Array(_) => {
                        return Err("unsupported type list (union types not supported)".to_string());
                    }
                    _ => {
                        return Err("invalid 'type' field: must be a string".to_string());
                    }
                }
            }

            // Check "format"
            if let Some(format_val) = map.get("format") {
                match format_val {
                    Value::String(f) => {
                        if f != "date-time" {
                            return Err(format!(
                                "unsupported format '{f}', only 'date-time' is supported"
                            ));
                        }
                    }
                    _ => return Err("invalid 'format' field: must be a string".to_string()),
                }
            }

            // Check "enum"
            if let Some(enum_val) = map.get("enum") {
                match enum_val {
                    Value::Array(items) => {
                        if items.is_empty() {
                            return Err("empty enum is not supported".to_string());
                        }
                        for item in items {
                            if !item.is_string() {
                                return Err("non-string enum variant is not supported".to_string());
                            }
                        }
                    }
                    _ => {
                        return Err("invalid 'enum' field: must be an array of strings".to_string());
                    }
                }
            }

            // Check "description"
            if let Some(desc_val) = map.get("description")
                && !desc_val.is_string()
            {
                return Err("invalid 'description' field: must be a string".to_string());
            }

            // Check "required"
            if let Some(req_val) = map.get("required") {
                match req_val {
                    Value::Array(items) => {
                        for item in items {
                            if !item.is_string() {
                                return Err(
                                    "invalid 'required' field: items must be strings".to_string()
                                );
                            }
                        }
                    }
                    _ => return Err("invalid 'required' field: must be an array".to_string()),
                }
            }

            // Recursively check "properties"
            if let Some(props_val) = map.get("properties") {
                match props_val {
                    Value::Object(props) => {
                        for (_prop_name, prop_schema) in props {
                            is_schema_supported(prop_schema)?;
                        }
                    }
                    _ => return Err("invalid 'properties' field: must be an object".to_string()),
                }
            }

            // Recursively check "items" (must be a single schema object, not an array of schemas)
            if let Some(items_val) = map.get("items") {
                match items_val {
                    Value::Object(_) => {
                        is_schema_supported(items_val)?;
                    }
                    Value::Array(_) => {
                        return Err("tuple schemas in 'items' are not supported".to_string());
                    }
                    _ => return Err("invalid 'items' field: must be a schema object".to_string()),
                }
            }

            Ok(())
        }
        Value::Bool(_) => Err("boolean schemas are not supported".to_string()),
        _ => Err("schema must be a JSON object".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_supported_basic_schemas() {
        assert!(
            is_schema_supported(&json!({
                "type": "string"
            }))
            .is_ok()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "format": "date-time"
            }))
            .is_ok()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "integer"
            }))
            .is_ok()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "number"
            }))
            .is_ok()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "boolean"
            }))
            .is_ok()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "enum": ["a", "b"]
            }))
            .is_ok()
        );
    }

    #[test]
    fn test_unsupported_keywords() {
        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "$ref": "#/definitions/Foo"
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "oneOf": [{"type": "string"}]
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "anyOf": [{"type": "string"}]
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "allOf": [{"type": "string"}]
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "pattern": "^[a-z]+$"
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "integer",
                "minimum": 1
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "default": "abc"
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "const": "abc"
            }))
            .is_err()
        );
    }

    #[test]
    fn test_unsupported_formats_and_types() {
        assert!(
            is_schema_supported(&json!({
                "type": "string",
                "format": "email"
            }))
            .is_err()
        );

        assert!(
            is_schema_supported(&json!({
                "type": ["string", "null"]
            }))
            .is_err()
        );
    }
}
