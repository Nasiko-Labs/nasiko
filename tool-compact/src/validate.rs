//! Argument validation against the original schema. Strict where JSON Schema is strict:
//! `int` rejects `1.0` and `"1"`, `?` means optional (not nullable), enums match exactly,
//! unknown keys are rejected only in closed objects. Formats get a light structural check.

use serde_json::Value;

use crate::schema::{Format, Kind, Node, normalize_tool};
use crate::{Error, ToolDef};

/// Check a call's `arguments` against `tool`'s schema.
pub fn validate_arguments(tool: &ToolDef, args: &Value) -> Result<(), Error> {
    let invalid = |reason: String| Error::InvalidArguments {
        tool: tool.name.clone(),
        reason,
    };
    // A schema we cannot read cannot vouch for any call.
    let schema = normalize_tool(tool).map_err(|e| invalid(e.to_string()))?;
    match schema {
        None => match args {
            Value::Object(m) if m.is_empty() => Ok(()),
            _ => Err(invalid("tool takes no arguments".into())),
        },
        Some(node) => check(&node, args, "$").map_err(invalid),
    }
}

pub(crate) fn check(node: &Node, v: &Value, path: &str) -> Result<(), String> {
    let ok = match (&node.kind, v) {
        (Kind::String(f), Value::String(s)) => f.is_none_or(|f| format_ok(f, s)),
        (Kind::Integer, Value::Number(n)) => n.is_i64() || n.is_u64(),
        (Kind::Number, Value::Number(_)) | (Kind::Boolean, Value::Bool(_)) => true,
        (Kind::Enum { values, .. }, _) => values.contains(v),
        (Kind::Array(items), Value::Array(a)) => {
            for (i, x) in a.iter().enumerate() {
                check(items, x, &format!("{path}[{i}]"))?;
            }
            true
        }
        (Kind::Object(o), Value::Object(m)) => {
            if let Some(k) = o.required.iter().find(|k| !m.contains_key(*k)) {
                return Err(format!("{path}.{k}: required field missing"));
            }
            for (k, x) in m {
                match o.properties.get(k) {
                    Some(n) => check(n, x, &format!("{path}.{k}"))?,
                    None if o.closed => return Err(format!("{path}.{k}: unknown field")),
                    None => {}
                }
            }
            true
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: {v} does not match the schema"))
    }
}

fn format_ok(f: Format, s: &str) -> bool {
    match f {
        Format::Date => is_date(s),
        Format::DateTime => s
            .split_once(['T', 't', ' '])
            .is_some_and(|(d, t)| is_date(d) && is_time(t)),
        Format::Email => {
            s.split_once('@')
                .is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.contains('@'))
                && !s.chars().any(char::is_whitespace)
        }
        Format::Uri => {
            s.split_once(':').is_some_and(|(scheme, rest)| {
                !rest.is_empty()
                    && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                    && scheme
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            }) && !s.chars().any(char::is_whitespace)
        }
    }
}

fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `YYYY-MM-DD` with a real month and day range.
fn is_date(s: &str) -> bool {
    let mut p = s.split('-');
    match (p.next(), p.next(), p.next(), p.next()) {
        (Some(y), Some(m), Some(d), None) => {
            digits(y, 4).is_some()
                && digits(m, 2).is_some_and(|m| (1..=12).contains(&m))
                && digits(d, 2).is_some_and(|d| (1..=31).contains(&d))
        }
        _ => false,
    }
}

/// `HH:MM[:SS[.frac]]` then optional `Z` or `±HH:MM`.
fn is_time(s: &str) -> bool {
    let (clock, offset) = match s.find(['Z', 'z', '+', '-']) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    let offset_ok = match offset {
        "" | "Z" | "z" => true,
        o => o.get(1..).is_some_and(|hm| hm_ok(hm, false)),
    };
    let (hms, frac) = clock.split_once('.').unwrap_or((clock, "0"));
    offset_ok && hm_ok(hms, true) && !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit())
}

fn hm_ok(s: &str, seconds: bool) -> bool {
    let mut p = s.split(':');
    let (h, m, sec, rest) = (p.next(), p.next(), p.next(), p.next());
    let sec_ok = match sec {
        None => true,
        Some(x) => seconds && digits(x, 2).is_some_and(|x| x <= 60),
    };
    rest.is_none()
        && sec_ok
        && h.and_then(|h| digits(h, 2)).is_some_and(|h| h <= 23)
        && m.and_then(|m| digits(m, 2)).is_some_and(|m| m <= 59)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool() -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "score": {"type": "number"},
                    "visibility": {"type": "string", "enum": ["public", "private"]},
                    "attendees": {"type": "array", "items": {"type": "string", "format": "email"}},
                    "opts": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {"day": {"type": "string", "format": "date"}, "url": {"type": "string", "format": "uri"}},
                        "required": ["day"]
                    }
                },
                "required": ["title", "start"]
            })),
        }
    }

    fn base() -> Value {
        json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30"})
    }

    fn with(k: &str, v: Value) -> Value {
        let mut b = base();
        b[k] = v;
        b
    }

    #[test]
    fn valid_calls_pass() {
        let t = tool();
        for args in [
            base(),
            with("duration_min", json!(30)),
            with("score", json!(1)),
            with("score", json!(1.5)),
            with("visibility", json!("private")),
            with("attendees", json!(["a@b.co"])),
            with("extra", json!("open object allows unknown keys")),
            with("opts", json!({"day": "2026-10-04", "url": "https://x.y/z"})),
            with("start", json!("2026-10-04T10:00Z")),
            with("start", json!("2026-10-04T10:00:00.5")),
        ] {
            assert!(validate_arguments(&t, &args).is_ok(), "{args}");
        }
    }

    #[test]
    fn invalid_calls_fail() {
        let t = tool();
        let mut missing = base();
        missing.as_object_mut().unwrap().remove("title");
        for args in [
            missing,
            json!([]),
            json!(null),
            with("title", json!(null)),
            with("title", json!(5)),
            with("duration_min", json!(1.0)),
            with("duration_min", json!("1")),
            with("score", json!("1")),
            with("visibility", json!("secret")),
            with("visibility", json!("Public")),
            with("attendees", json!("a@b.co")),
            with("attendees", json!(["not an email"])),
            with("opts", json!({})),
            with("opts", json!({"day": "2026-10-04", "zzz": 1})),
            with("opts", json!({"day": "2026-13-04"})),
            with("opts", json!({"day": "2026-10-04", "url": "no scheme"})),
            with("start", json!("next monday 3pm")),
            with("start", json!("2026-10-04T25:00:00")),
            with("start", json!("2026-10-04T10:00:00+0530")),
        ] {
            let e = validate_arguments(&t, &args).unwrap_err();
            assert_eq!(e.as_label(), "invalid_arguments", "{args}");
        }
    }

    #[test]
    fn integer_enum_is_exact_and_no_params_means_empty_object() {
        let t = ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(
                json!({"type": "object", "properties": {"n": {"type": "integer", "enum": [1, 2]}}}),
            ),
        };
        assert!(validate_arguments(&t, &json!({"n": 1})).is_ok());
        assert!(validate_arguments(&t, &json!({"n": 1.0})).is_err());
        assert!(validate_arguments(&t, &json!({"n": 3})).is_err());
        let none = ToolDef {
            parameters: None,
            ..t
        };
        assert!(validate_arguments(&none, &json!({})).is_ok());
        assert!(validate_arguments(&none, &json!({"a": 1})).is_err());
    }
}
