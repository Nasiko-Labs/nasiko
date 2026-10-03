//! JSON Schema validation for decoded tool-call arguments.
//!
//! # What is validated
//!
//! - `type`: `string`, `integer`, `number`, `boolean`, `array`, `object`.
//! - `properties` + `required`: all required fields must be present; each field
//!   is recursively validated.
//! - `additionalProperties: false`: extra properties are rejected.
//! - `enum`: value must appear in the listed values.
//! - `items`: array items are recursively validated.
//! - Nullable: `"type": ["T", "null"]` allows `null` for that field.
//! - `format: date-time` and `format: date` are validated as RFC 3339 syntax
//!   (syntactic check only; semantic correctness such as leap-second validity
//!   is out of scope).
//!
//! # What is NOT validated (unsupported, recorded at encode time)
//!
//! `$ref`, `oneOf`/`anyOf`/`allOf`/`not`, `patternProperties`, `if`/`then`/`else`,
//! numeric constraints (`minimum`, `maximum`, …), string constraints
//! (`minLength`, `maxLength`, `pattern`), `default`. Tools using these features
//! are kept in native form by the encoder; the validator never sees them.
//!
//! # Key-order policy
//!
//! `arguments` in the returned [`crate::types::ToolCall`] preserves the key
//! order as emitted by the model. No re-sorting is performed.

use serde_json::Value;

use crate::{Error, Result};

/// Validate `args` against `schema` and return the error if invalid.
///
/// `tool_name` is used only for error messages.
pub(crate) fn validate(tool_name: &str, args: &Value, schema: &Value) -> Result<()> {
    validate_value(tool_name, args, schema)
}

fn validate_value(tool_name: &str, value: &Value, schema: &Value) -> Result<()> {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return Ok(()), // no schema constraints
    };

    // Detect nullable types.
    let nullable = is_nullable(obj);

    // Allow null if the schema permits it.
    if value.is_null() {
        if nullable {
            return Ok(());
        }
        return Err(invalid(tool_name, "null value where null is not allowed"));
    }

    // `enum` check.
    if let Some(enum_vals) = obj.get("enum").and_then(Value::as_array) {
        if !enum_vals.iter().any(|v| v == value) {
            return Err(invalid(
                tool_name,
                &format!(
                    "value {:?} is not in enum {:?}",
                    value,
                    enum_vals
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        return Ok(());
    }

    // `type` check.
    let declared_type = primary_type(obj);

    match declared_type {
        Some("string") => {
            let s = value.as_str().ok_or_else(|| {
                invalid(tool_name, &format!("expected string, got {}", type_name(value)))
            })?;
            // format validation.
            if let Some(fmt) = obj.get("format").and_then(Value::as_str) {
                match fmt {
                    "date-time" => validate_datetime(tool_name, s)?,
                    "date" => validate_date(tool_name, s)?,
                    _ => {} // unknown formats pass through
                }
            }
        }
        Some("integer") => {
            // JSON numbers are all f64 under serde_json; integers must have no
            // fractional part.
            match value {
                Value::Number(n) => {
                    let f = n.as_f64().unwrap_or(f64::NAN);
                    if f.fract() != 0.0 || f.is_nan() || f.is_infinite() {
                        return Err(invalid(
                            tool_name,
                            &format!("expected integer, got fractional number {}", n),
                        ));
                    }
                }
                Value::String(_) => {
                    return Err(invalid(
                        tool_name,
                        &format!("expected integer, got string {:?}", value.as_str().unwrap()),
                    ));
                }
                _ => {
                    return Err(invalid(
                        tool_name,
                        &format!("expected integer, got {}", type_name(value)),
                    ));
                }
            }
        }
        Some("number") => {
            if !value.is_number() {
                return Err(invalid(
                    tool_name,
                    &format!("expected number, got {}", type_name(value)),
                ));
            }
        }
        Some("boolean") => {
            if !value.is_boolean() {
                return Err(invalid(
                    tool_name,
                    &format!("expected boolean, got {}", type_name(value)),
                ));
            }
        }
        Some("array") => {
            let arr = value.as_array().ok_or_else(|| {
                invalid(tool_name, &format!("expected array, got {}", type_name(value)))
            })?;
            if let Some(items_schema) = obj.get("items") {
                for (i, item) in arr.iter().enumerate() {
                    validate_value(tool_name, item, items_schema).map_err(|e| {
                        invalid(tool_name, &format!("array[{}]: {}", i, err_reason(&e)))
                    })?;
                }
            }
        }
        Some("object") => {
            let map = value.as_object().ok_or_else(|| {
                invalid(tool_name, &format!("expected object, got {}", type_name(value)))
            })?;

            let properties = obj.get("properties").and_then(Value::as_object);
            let required: Vec<&str> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();

            // Check required fields.
            for req in &required {
                if !map.contains_key(*req) {
                    return Err(invalid(
                        tool_name,
                        &format!("missing required field {:?}", req),
                    ));
                }
            }

            // Check additional properties.
            let additional_properties_false = obj
                .get("additionalProperties")
                .and_then(Value::as_bool)
                .map(|b| !b) // false means reject extras
                .unwrap_or(false);

            if additional_properties_false {
                let allowed: std::collections::HashSet<&str> = properties
                    .map(|p| p.keys().map(|k| k.as_str()).collect())
                    .unwrap_or_default();
                for key in map.keys() {
                    if !allowed.contains(key.as_str()) {
                        return Err(invalid(
                            tool_name,
                            &format!("additional property {:?} is not allowed", key),
                        ));
                    }
                }
            }

            // Validate each property.
            if let Some(props) = properties {
                for (key, val) in map {
                    if let Some(prop_schema) = props.get(key) {
                        validate_value(tool_name, val, prop_schema)?;
                    }
                    // Properties without a declared schema are allowed unless
                    // additionalProperties:false (handled above).
                }
            }
        }
        Some(other) => {
            return Err(invalid(
                tool_name,
                &format!("schema declares unknown type {:?}", other),
            ));
        }
        None => {
            // No type declared: any value is valid unless `enum` (handled above).
        }
    }

    Ok(())
}

/// Extract the primary (non-null) type from a schema.
fn primary_type(obj: &serde_json::Map<String, Value>) -> Option<&str> {
    match obj.get("type")? {
        Value::String(s) => Some(s.as_str()),
        Value::Array(arr) => arr
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null"),
        _ => None,
    }
}

/// Detect if a schema allows `null`.
fn is_nullable(obj: &serde_json::Map<String, Value>) -> bool {
    if let Some(arr) = obj.get("type").and_then(Value::as_array) {
        return arr.iter().any(|v| v.as_str() == Some("null"));
    }
    false
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn invalid(tool_name: &str, reason: &str) -> Error {
    Error::InvalidArguments {
        tool: tool_name.to_string(),
        reason: reason.to_string(),
    }
}

fn err_reason(e: &Error) -> String {
    match e {
        Error::InvalidArguments { reason, .. } => reason.clone(),
        other => other.to_string(),
    }
}

/// RFC 3339 date-time syntactic check: `YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)`.
///
/// This is a syntactic check only; it does not verify leap seconds or
/// calendar validity (e.g. Feb 30).
fn validate_datetime(tool_name: &str, s: &str) -> Result<()> {
    // Minimum: `1970-01-01T00:00:00Z` = 20 chars.
    if s.len() < 20 {
        return Err(invalid(
            tool_name,
            &format!("date-time {:?} is too short", s),
        ));
    }
    // Check T separator at position 10.
    let bytes = s.as_bytes();
    if bytes[10] != b'T' && bytes[10] != b't' {
        return Err(invalid(
            tool_name,
            &format!("date-time {:?} missing T separator", s),
        ));
    }
    // Validate date portion `YYYY-MM-DD`.
    validate_date_portion(tool_name, &s[..10])?;
    // Validate time portion.
    validate_time_portion(tool_name, &s[11..])?;
    Ok(())
}

/// RFC 3339 date syntactic check: `YYYY-MM-DD`.
fn validate_date(tool_name: &str, s: &str) -> Result<()> {
    if s.len() != 10 {
        return Err(invalid(
            tool_name,
            &format!("date {:?} must be exactly 10 chars (YYYY-MM-DD)", s),
        ));
    }
    validate_date_portion(tool_name, s)
}

fn validate_date_portion(tool_name: &str, s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 {
        return Err(invalid(tool_name, "date too short"));
    }
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(invalid(
            tool_name,
            &format!("date {:?} missing dashes", s),
        ));
    }
    if !bytes[..4].iter().all(|b| b.is_ascii_digit())
        || !bytes[5..7].iter().all(|b| b.is_ascii_digit())
        || !bytes[8..10].iter().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(
            tool_name,
            &format!("date {:?} contains non-digit", s),
        ));
    }
    Ok(())
}

fn validate_time_portion(tool_name: &str, s: &str) -> Result<()> {
    // `HH:MM:SS` then optional `.fff` then `Z` or `+HH:MM` or `-HH:MM`.
    if s.len() < 8 {
        return Err(invalid(
            tool_name,
            &format!("time portion {:?} too short", s),
        ));
    }
    let bytes = s.as_bytes();
    if bytes[2] != b':' || bytes[5] != b':' {
        return Err(invalid(
            tool_name,
            &format!("time portion {:?} missing colons", s),
        ));
    }
    if !bytes[..2].iter().all(|b| b.is_ascii_digit())
        || !bytes[3..5].iter().all(|b| b.is_ascii_digit())
        || !bytes[6..8].iter().all(|b| b.is_ascii_digit())
    {
        return Err(invalid(
            tool_name,
            &format!("time portion {:?} contains non-digit", s),
        ));
    }
    // Remainder (after HH:MM:SS).
    let rest = &s[8..];
    let rest = if let Some(stripped) = rest.strip_prefix('.') {
        // Skip fractional seconds.
        let end = stripped
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        &stripped[end..]
    } else {
        rest
    };
    // Must be `Z` or `±HH:MM`.
    if rest == "Z" || rest == "z" {
        return Ok(());
    }
    if rest.len() == 6
        && (rest.starts_with('+') || rest.starts_with('-'))
        && rest.as_bytes()[3] == b':'
        && rest[1..3].bytes().all(|b| b.is_ascii_digit())
        && rest[4..6].bytes().all(|b| b.is_ascii_digit())
    {
        return Ok(());
    }
    Err(invalid(
        tool_name,
        &format!("time zone in {:?} is malformed", s),
    ))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(tool: &str, args: Value, schema: Value) {
        validate(tool, &args, &schema).expect("should be valid");
    }

    fn err(tool: &str, args: Value, schema: Value) {
        validate(tool, &args, &schema).expect_err("should be invalid");
    }

    #[test]
    fn string_type_valid() {
        ok("f", json!("hello"), json!({"type":"string"}));
    }

    #[test]
    fn string_for_integer_is_error() {
        err("f", json!("30"), json!({"type":"integer"}));
    }

    #[test]
    fn integer_valid() {
        ok("f", json!(42), json!({"type":"integer"}));
    }

    #[test]
    fn fractional_for_integer_is_error() {
        err("f", json!(3.14), json!({"type":"integer"}));
    }

    #[test]
    fn number_valid_for_float() {
        ok("f", json!(3.14), json!({"type":"number"}));
    }

    #[test]
    fn boolean_valid() {
        ok("f", json!(true), json!({"type":"boolean"}));
    }

    #[test]
    fn array_validates_items() {
        ok(
            "f",
            json!(["a", "b"]),
            json!({"type":"array","items":{"type":"string"}}),
        );
        err(
            "f",
            json!([1, 2]),
            json!({"type":"array","items":{"type":"string"}}),
        );
    }

    #[test]
    fn required_field_missing_is_error() {
        err(
            "f",
            json!({}),
            json!({
                "type":"object",
                "properties":{"name":{"type":"string"}},
                "required":["name"]
            }),
        );
    }

    #[test]
    fn additional_property_rejected_when_false() {
        err(
            "f",
            json!({"name":"Alice","extra":"x"}),
            json!({
                "type":"object",
                "properties":{"name":{"type":"string"}},
                "additionalProperties": false
            }),
        );
    }

    #[test]
    fn enum_valid() {
        ok(
            "f",
            json!("active"),
            json!({"enum":["active","inactive"]}),
        );
    }

    #[test]
    fn enum_out_of_range() {
        err(
            "f",
            json!("deleted"),
            json!({"enum":["active","inactive"]}),
        );
    }

    #[test]
    fn null_allowed_in_nullable_schema() {
        ok(
            "f",
            json!(null),
            json!({"type":["string","null"]}),
        );
    }

    #[test]
    fn null_rejected_in_non_nullable_schema() {
        err("f", json!(null), json!({"type":"string"}));
    }

    #[test]
    fn valid_datetime() {
        ok("f", json!("2026-10-03T10:00:00+05:30"), json!({"type":"string","format":"date-time"}));
        ok("f", json!("2026-10-03T00:00:00Z"), json!({"type":"string","format":"date-time"}));
        ok("f", json!("2026-10-03T10:00:00.123Z"), json!({"type":"string","format":"date-time"}));
    }

    #[test]
    fn invalid_datetime() {
        err("f", json!("not-a-date"), json!({"type":"string","format":"date-time"}));
        err("f", json!("2026-10-03"), json!({"type":"string","format":"date-time"}));
    }

    #[test]
    fn valid_date() {
        ok("f", json!("2026-10-03"), json!({"type":"string","format":"date"}));
    }

    #[test]
    fn invalid_date() {
        err("f", json!("2026/10/03"), json!({"type":"string","format":"date"}));
        err("f", json!("not-a-date"), json!({"type":"string","format":"date"}));
    }

    #[test]
    fn nested_object_validates_recursively() {
        let schema = json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    },
                    "required": ["city"]
                }
            },
            "required": ["location"]
        });
        ok("f", json!({"location":{"city":"London"}}), schema.clone());
        err("f", json!({"location":{}}), schema);
    }
}
