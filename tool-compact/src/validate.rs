//! Argument validation against the tool's **original** JSON Schema.
//!
//! This module knows nothing about the compact text format: it takes a parsed
//! [`Value`] and the schema the client sent, and answers "is this exactly what the
//! client asked for?". Keeping it separate from the parser means a call is checked
//! against the real schema, never against our compact rendering of it.
//!
//! # Rules
//!
//! Only the keywords in [`SUPPORTED_KEYWORDS`] are understood. Any other keyword is
//! an error rather than being ignored: validating against a rule we do not
//! understand would let invalid calls through.
//!
//! | keyword       | rule                                                                  |
//! |---------------|-----------------------------------------------------------------------|
//! | `type`        | `string`, `integer` (whole JSON number only; `1.0` is rejected), `number`, `boolean`, `array`, `object`. Absent: no type check. |
//! | `enum`        | value must equal one of the listed values                             |
//! | `format`      | `date-time` only: RFC 3339 with a UTC offset (`2026-10-05T15:00:00+05:30`) |
//! | `properties`  | each present key is validated; **keys not listed are rejected**       |
//! | `required`    | each listed key must be present (`null` does not count as a value)   |
//! | `items`       | every element is validated against the single item schema            |
//! | `description` | annotation only                                                       |
//!
//! An object schema without `properties` accepts any keys, matching JSON Schema.
//! `null` is never accepted, since no supported `type` admits it.

use serde_json::{Map, Value};

use crate::error::{CompactError, Result};
use crate::schema::SUPPORTED_KEYWORDS;

/// Validate tool-call `args` against the tool's parameter `schema`.
///
/// `args` must be a JSON object, as tool arguments always are. On failure the
/// returned [`CompactError::InvalidArguments`] has an empty `tool`; the decoder,
/// which knows the tool name, fills it in.
pub fn validate_arguments(schema: &Value, args: &Value) -> Result<()> {
    if !args.is_object() {
        return Err(invalid(format!(
            "arguments must be a JSON object, got {}",
            kind(args)
        )));
    }
    check(schema, args, "$")
}

fn check(schema: &Value, value: &Value, path: &str) -> Result<()> {
    let schema = match schema {
        Value::Object(map) => map,
        _ => return Err(invalid_schema(path, "schema must be a JSON object")),
    };

    if let Some(key) = schema
        .keys()
        .find(|k| !SUPPORTED_KEYWORDS.contains(&k.as_str()))
    {
        return Err(invalid_schema(
            path,
            &format!("unsupported keyword '{key}'"),
        ));
    }

    if let Some(ty) = schema.get("type") {
        check_type(ty, value, path)?;
    }
    if let Some(variants) = schema.get("enum") {
        check_enum(variants, value, path)?;
    }
    if let Some(format) = schema.get("format") {
        check_format(format, value, path)?;
    }
    if let Value::Object(obj) = value {
        check_object(schema, obj, path)?;
    }
    if let (Some(items), Value::Array(elements)) = (schema.get("items"), value) {
        for (i, element) in elements.iter().enumerate() {
            check(items, element, &format!("{path}[{i}]"))?;
        }
    }
    Ok(())
}

fn check_type(ty: &Value, value: &Value, path: &str) -> Result<()> {
    let ty = ty
        .as_str()
        .ok_or_else(|| invalid_schema(path, "'type' must be a string"))?;
    let ok = match ty {
        "string" => value.is_string(),
        // serde_json stores `1.0` as f64; only i64/u64 are whole JSON numbers.
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        other => return Err(invalid_schema(path, &format!("unsupported type '{other}'"))),
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(format!(
            "{path}: expected {ty}, got {}",
            kind(value)
        )))
    }
}

fn check_enum(variants: &Value, value: &Value, path: &str) -> Result<()> {
    let variants = variants
        .as_array()
        .ok_or_else(|| invalid_schema(path, "'enum' must be an array"))?;
    if variants.contains(value) {
        Ok(())
    } else {
        Err(invalid(format!(
            "{path}: {value} is not one of {}",
            Value::Array(variants.clone())
        )))
    }
}

fn check_format(format: &Value, value: &Value, path: &str) -> Result<()> {
    match format.as_str() {
        Some("date-time") => match value.as_str() {
            Some(s) if chrono::DateTime::parse_from_rfc3339(s).is_ok() => Ok(()),
            Some(s) => Err(invalid(format!(
                "{path}: '{s}' is not an RFC 3339 date-time with offset"
            ))),
            // A non-string is reported by the `type` check; format alone constrains strings only.
            None => Ok(()),
        },
        Some(other) => Err(invalid_schema(
            path,
            &format!("unsupported format '{other}'"),
        )),
        None => Err(invalid_schema(path, "'format' must be a string")),
    }
}

fn check_object(schema: &Map<String, Value>, obj: &Map<String, Value>, path: &str) -> Result<()> {
    if let Some(required) = schema.get("required") {
        let required = required
            .as_array()
            .ok_or_else(|| invalid_schema(path, "'required' must be an array"))?;
        for key in required {
            let key = key
                .as_str()
                .ok_or_else(|| invalid_schema(path, "'required' entries must be strings"))?;
            if !obj.contains_key(key) {
                return Err(invalid(format!("{path}: missing required field '{key}'")));
            }
        }
    }

    let Some(properties) = schema.get("properties") else {
        return Ok(());
    };
    let properties = properties
        .as_object()
        .ok_or_else(|| invalid_schema(path, "'properties' must be an object"))?;
    for (key, value) in obj {
        let child = format!("{path}.{key}");
        match properties.get(key) {
            Some(prop_schema) => check(prop_schema, value, &child)?,
            None => return Err(invalid(format!("{path}: unknown field '{key}'"))),
        }
    }
    Ok(())
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn invalid(reason: String) -> CompactError {
    CompactError::InvalidArguments {
        tool: String::new(),
        reason,
    }
}

fn invalid_schema(path: &str, reason: &str) -> CompactError {
    CompactError::InvalidSchema {
        tool: String::new(),
        reason: format!("{path}: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })
    }

    fn reason(result: Result<()>) -> String {
        match result {
            Err(CompactError::InvalidArguments { reason, .. }) => reason,
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[test]
    fn accepts_valid_minimal_and_full_calls() {
        let s = calendar();
        assert!(
            validate_arguments(
                &s,
                &json!({"title": "x", "start": "2026-10-05T15:00:00+05:30"})
            )
            .is_ok()
        );
        assert!(
            validate_arguments(
                &s,
                &json!({
                    "title": "Retro", "start": "2026-10-04T10:00:00Z", "duration_min": 30,
                    "attendees": ["a@example.com"], "visibility": "private"
                })
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_missing_required_field() {
        let r = reason(validate_arguments(
            &calendar(),
            &json!({"start": "2026-10-05T15:00:00+05:30"}),
        ));
        assert!(r.contains("missing required field 'title'"), "{r}");
    }

    #[test]
    fn rejects_enum_violation() {
        let r = reason(validate_arguments(
            &calendar(),
            &json!({
                "title": "x", "start": "2026-10-05T15:00:00+05:30", "visibility": "secret"
            }),
        ));
        assert!(r.contains("visibility"), "{r}");
    }

    #[test]
    fn rejects_wrong_types() {
        let s = calendar();
        let base = || json!({"title": "x", "start": "2026-10-05T15:00:00+05:30"});
        for (key, bad) in [
            ("title", json!(5)),
            ("duration_min", json!("30")),
            ("duration_min", json!(30.5)),
            ("duration_min", json!(30.0)),
            ("duration_min", json!(true)),
            ("attendees", json!("a@example.com")),
            ("duration_min", Value::Null),
        ] {
            let mut args = base();
            args[key] = bad.clone();
            assert!(
                validate_arguments(&s, &args).is_err(),
                "{key}={bad} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_bad_array_element_with_path() {
        let r = reason(validate_arguments(
            &calendar(),
            &json!({
                "title": "x", "start": "2026-10-05T15:00:00+05:30", "attendees": ["a", 7]
            }),
        ));
        assert!(r.contains("$.attendees[1]"), "{r}");
    }

    #[test]
    fn rejects_unknown_field() {
        let r = reason(validate_arguments(
            &calendar(),
            &json!({
                "title": "x", "start": "2026-10-05T15:00:00+05:30", "titel": "typo"
            }),
        ));
        assert!(r.contains("unknown field 'titel'"), "{r}");
    }

    #[test]
    fn date_time_requires_rfc3339_with_offset() {
        let s = calendar();
        for bad in [
            "2026-10-05T15:00:00",
            "2026-10-05",
            "Monday 3pm",
            "2026-13-05T15:00:00Z",
        ] {
            assert!(
                validate_arguments(&s, &json!({"title": "x", "start": bad})).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_non_object_arguments() {
        for bad in [json!([]), json!("x"), json!(1), Value::Null] {
            assert!(validate_arguments(&calendar(), &bad).is_err());
        }
    }

    #[test]
    fn validates_nested_objects_and_arrays() {
        let s = json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}, "zip": {"type": "string"}},
                    "required": ["city"]
                },
                "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}},
                "people": {"type": "array", "items": {
                    "type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]
                }}
            }
        });
        assert!(validate_arguments(&s, &json!({
            "address": {"city": "Pune"}, "matrix": [[1, 2], []], "people": [{"name": "Riya"}]
        }))
        .is_ok());
        assert!(validate_arguments(&s, &json!({"address": {"zip": "411001"}})).is_err());
        assert!(validate_arguments(&s, &json!({"matrix": [[1, "2"]]})).is_err());
        assert!(validate_arguments(&s, &json!({"people": [{}]})).is_err());
    }

    #[test]
    fn missing_type_means_no_type_check() {
        let s = json!({"type": "object", "properties": {"anything": {"description": "free"}}});
        for v in [json!(1), json!("x"), json!([1]), json!({"k": 1})] {
            assert!(validate_arguments(&s, &json!({"anything": v})).is_ok());
        }
    }

    #[test]
    fn object_without_properties_accepts_any_keys() {
        let s = json!({"type": "object"});
        assert!(validate_arguments(&s, &json!({"a": 1, "b": [true]})).is_ok());
    }

    #[test]
    fn unsupported_schema_keyword_is_an_error_not_ignored() {
        let s = json!({"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}});
        assert!(matches!(
            validate_arguments(&s, &json!({"n": 0})),
            Err(CompactError::InvalidSchema { .. })
        ));
    }
}
