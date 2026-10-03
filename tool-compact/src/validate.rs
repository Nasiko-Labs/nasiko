//! Validation of decoded arguments against the original JSON Schema (supported subset).

use serde_json::{Map, Value};

/// Returns `Err(reason)` when `value` violates `schema`.
pub(crate) fn validate(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
    let Some(obj) = schema.as_object() else {
        return Ok(());
    };
    if let Some(options) = obj.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!(
                "{path}: value is not one of the allowed enum values"
            ));
        }
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("object") => validate_object(value, obj, path),
        Some("array") => {
            let items = value
                .as_array()
                .ok_or_else(|| format!("{path}: expected array"))?;
            if let Some(item_schema) = obj.get("items") {
                for (i, item) in items.iter().enumerate() {
                    validate(item, item_schema, &format!("{path}[{i}]"))?;
                }
            }
            Ok(())
        }
        Some("string") => {
            let s = value
                .as_str()
                .ok_or_else(|| format!("{path}: expected string"))?;
            match obj.get("format").and_then(Value::as_str) {
                Some("date-time") if !is_date_time(s) => {
                    Err(format!("{path}: not an ISO 8601 date-time"))
                }
                Some("date") if !is_date(s) => Err(format!("{path}: not an ISO 8601 date")),
                _ => Ok(()),
            }
        }
        Some("integer") => ok_if(value.is_i64() || value.is_u64(), path, "integer"),
        Some("number") => ok_if(value.is_number(), path, "number"),
        Some("boolean") => ok_if(value.is_boolean(), path, "boolean"),
        Some("null") => ok_if(value.is_null(), path, "null"),
        _ => Ok(()),
    }
}

fn ok_if(cond: bool, path: &str, expected: &str) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(format!("{path}: expected {expected}"))
    }
}

fn validate_object(value: &Value, schema: &Map<String, Value>, path: &str) -> Result<(), String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{path}: expected object"))?;
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !map.contains_key(name) {
                return Err(format!("{path}: missing required field `{name}`"));
            }
        }
    }
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };
    let extra_ok = schema.get("additionalProperties") == Some(&Value::Bool(true));
    for (key, val) in map {
        match props.get(key) {
            Some(prop_schema) => validate(val, prop_schema, &format!("{path}.{key}"))?,
            None if extra_ok => {}
            None => return Err(format!("{path}: unknown field `{key}`")),
        }
    }
    Ok(())
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = [0, 1, 2, 3, 5, 6, 8, 9]
        .iter()
        .all(|&i| b[i].is_ascii_digit());
    if !digits {
        return false;
    }
    let month: u8 = s[5..7].parse().unwrap_or(0);
    let day: u8 = s[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// `YYYY-MM-DDTHH:MM:SS[.frac][Z|±HH:MM]`; the offset is optional.
fn is_date_time(s: &str) -> bool {
    let Some((date, time)) = s.split_once('T') else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let b = time.as_bytes();
    if b.len() < 8 {
        return false;
    }
    let shape =
        [0, 1, 3, 4, 6, 7].iter().all(|&i| b[i].is_ascii_digit()) && b[2] == b':' && b[5] == b':';
    if !shape {
        return false;
    }
    let hour: u8 = time[0..2].parse().unwrap_or(99);
    let minute: u8 = time[3..5].parse().unwrap_or(99);
    let second: u8 = time[6..8].parse().unwrap_or(99);
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let mut rest = &time[8..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &frac[n..];
    }
    match rest {
        "" | "Z" => true,
        _ => {
            let r = rest.as_bytes();
            r.len() == 6
                && (r[0] == b'+' || r[0] == b'-')
                && r[1].is_ascii_digit()
                && r[2].is_ascii_digit()
                && r[3] == b':'
                && r[4].is_ascii_digit()
                && r[5].is_ascii_digit()
                && rest[1..3].parse::<u8>().map_or(false, |h| h <= 23)
                && rest[4..6].parse::<u8>().map_or(false, |m| m <= 59)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        crate::testutil::tools()[0].parameters.clone().unwrap()
    }

    #[test]
    fn accepts_valid() {
        let v = json!({"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":30,"visibility":"private"});
        assert!(validate(&v, &schema(), "arguments").is_ok());
        let v = json!({"title":"x","start":"2026-01-01T10:00:00"});
        assert!(validate(&v, &schema(), "arguments").is_ok());
    }

    #[test]
    fn rejects_violations() {
        let bad = [
            json!({"start":"2026-10-05T15:00:00Z"}),
            json!({"title":"x","start":"2026-10-05T15:00:00Z","visibility":"secret"}),
            json!({"title":1,"start":"2026-10-05T15:00:00Z"}),
            json!({"title":"x","start":"tomorrow"}),
            json!({"title":"x","start":"2026-13-05T15:00:00Z"}),
            json!({"title":"x","start":"2026-10-05T15:00:00Z","duration_min":1.5}),
            json!({"title":"x","start":"2026-10-05T15:00:00Z","attendees":"a@b.c"}),
            json!({"title":"x","start":"2026-10-05T15:00:00Z","attendees":[1]}),
            json!({"title":"x","start":"2026-10-05T15:00:00Z","extra":true}),
            json!(["not", "an", "object"]),
        ];
        for v in bad {
            assert!(validate(&v, &schema(), "arguments").is_err(), "{v}");
        }
    }

    #[test]
    fn nested_objects_validate() {
        let s = json!({"type":"object","properties":{"a":{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}},"required":["a"]});
        assert!(validate(&json!({"a":{"n":1}}), &s, "p").is_ok());
        assert!(validate(&json!({"a":{}}), &s, "p").is_err());
        assert!(validate(&json!({"a":{"n":"1"}}), &s, "p").is_err());
    }
}
