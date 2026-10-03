//! Argument validation against [`Ty`]. Strict and fail-closed: nothing is coerced or dropped.

use serde_json::{Number, Value};

use crate::ty::{Bounds, Format, Obj, Ty};

/// Check `args` against a tool's parameters. The error names the offending path.
pub(crate) fn check_args(params: &Obj, args: &Value) -> Result<(), String> {
    check(&Ty::Obj(params.clone()), args, "")
}

fn check(ty: &Ty, v: &Value, path: &str) -> Result<(), String> {
    let at = || {
        if path.is_empty() {
            "arguments".to_string()
        } else {
            format!("`{path}`")
        }
    };
    let ok = match ty {
        Ty::Any => true,
        Ty::Str(format) => match v.as_str() {
            Some(s) => {
                if let Some(f) = format
                    && !format_ok(*f, s)
                {
                    return Err(format!("{}: {s:?} is not a valid {}", at(), f.keyword()));
                }
                true
            }
            None => false,
        },
        Ty::Int => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        Ty::Num => v.is_number(),
        Ty::Bool => v.is_boolean(),
        Ty::Null => v.is_null(),
        Ty::Enum(values) => {
            if !values.contains(v) {
                let allowed: Vec<String> = values.iter().map(Value::to_string).collect();
                return Err(format!(
                    "{}: {v} is not one of {}",
                    at(),
                    allowed.join(", ")
                ));
            }
            true
        }
        Ty::Arr(inner) => match v.as_array() {
            Some(items) => {
                for (i, item) in items.iter().enumerate() {
                    check(inner, item, &format!("{path}[{i}]"))?;
                }
                true
            }
            None => false,
        },
        Ty::Obj(obj) => match v.as_object() {
            Some(map) => {
                for f in &obj.fields {
                    let sub = join(path, &f.name);
                    match map.get(&f.name) {
                        Some(value) => check(&f.ty, value, &sub)?,
                        None if f.required => {
                            return Err(format!("missing required field `{sub}`"));
                        }
                        None => {}
                    }
                }
                if !obj.open
                    && let Some(k) = map
                        .keys()
                        .find(|k| !obj.fields.iter().any(|f| &f.name == *k))
                {
                    return Err(format!("unknown field `{}`", join(path, k)));
                }
                true
            }
            None => false,
        },
        Ty::Nullable(_) if v.is_null() => true,
        Ty::Nullable(inner) => return check(inner, v, path),
        Ty::Bounded(inner, b) => {
            check(inner, v, path)?;
            // A value for numbers; a length for strings (in characters) and arrays.
            let (measure, what) = match v {
                Value::String(s) => (s.chars().count() as f64, "length "),
                Value::Array(a) => (a.len() as f64, "length "),
                _ => (v.as_f64().unwrap_or(f64::NAN), ""),
            };
            return check_bounds(measure, b)
                .map_err(|limit| format!("{}: {what}{measure} is not {limit}", at()));
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{}: expected {}, got {}",
            at(),
            expected(ty),
            kind(v)
        ))
    }
}

/// `Err` names the violated limit, e.g. `>= 1`.
fn check_bounds(x: f64, b: &Bounds) -> Result<(), String> {
    if let Some(min) = b.min.as_ref().and_then(Number::as_f64) {
        let ok = if b.min_exclusive { x > min } else { x >= min };
        if !ok {
            return Err(format!(
                "{} {min}",
                if b.min_exclusive { ">" } else { ">=" }
            ));
        }
    }
    if let Some(max) = b.max.as_ref().and_then(Number::as_f64) {
        let ok = if b.max_exclusive { x < max } else { x <= max };
        if !ok {
            return Err(format!(
                "{} {max}",
                if b.max_exclusive { "<" } else { "<=" }
            ));
        }
    }
    Ok(())
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn format_ok(f: Format, s: &str) -> bool {
    match f {
        Format::DateTime => chrono::DateTime::parse_from_rfc3339(s).is_ok(),
        Format::Date => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
        Format::Email => s.split_once('@').is_some_and(|(user, host)| {
            !user.is_empty() && host.contains('.') && !s.contains(char::is_whitespace)
        }),
        Format::Uri => s.split_once(':').is_some_and(|(scheme, rest)| {
            !rest.is_empty()
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }),
    }
}

fn expected(ty: &Ty) -> &'static str {
    match ty {
        Ty::Any => "any value",
        Ty::Str(_) => "a string",
        Ty::Int => "an integer",
        Ty::Num => "a number",
        Ty::Bool => "a boolean",
        Ty::Null => "null",
        Ty::Enum(_) => "an allowed value",
        Ty::Arr(_) => "an array",
        Ty::Obj(_) => "an object",
        Ty::Nullable(_) => "a value or null",
        Ty::Bounded(inner, _) => expected(inner),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{Ty, from_schema};
    use serde_json::json;

    fn calendar() -> Obj {
        let Ty::Obj(o) = from_schema(
            &json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            }),
            "p",
        )
        .unwrap() else {
            panic!()
        };
        o
    }

    #[test]
    fn accepts_valid_arguments() {
        let ok = json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30",
                        "duration_min": 30, "attendees": ["a@b.co"], "visibility": "private"});
        assert_eq!(check_args(&calendar(), &ok), Ok(()));
        assert_eq!(
            check_args(
                &calendar(),
                &json!({"title": "x", "start": "2026-10-04T10:00:00Z", "duration_min": 30.0})
            ),
            Ok(())
        );
    }

    #[test]
    fn rejects_every_kind_of_violation() {
        let o = calendar();
        let cases = [
            (
                json!({"start": "2026-10-04T10:00:00Z"}),
                "missing required field `title`",
            ),
            (
                json!({"title": "x", "start": "2026-10-04T10:00:00Z", "visibility": "secret"}),
                "not one of",
            ),
            (
                json!({"title": "x", "start": "tomorrow"}),
                "not a valid datetime",
            ),
            (
                json!({"title": "x", "start": "2026-10-04T10:00:00Z", "duration_min": "30"}),
                "expected an integer",
            ),
            (
                json!({"title": "x", "start": "2026-10-04T10:00:00Z", "duration_min": 2.5}),
                "expected an integer",
            ),
            (
                json!({"title": "x", "start": "2026-10-04T10:00:00Z", "attendees": ["a", 1]}),
                "`attendees[1]`",
            ),
            (
                json!({"title": "x", "start": "2026-10-04T10:00:00Z", "room": "A"}),
                "unknown field `room`",
            ),
            (
                json!({"title": null, "start": "2026-10-04T10:00:00Z"}),
                "expected a string",
            ),
            (json!(["not", "an", "object"]), "expected an object"),
        ];
        for (args, want) in cases {
            let err = check_args(&o, &args).unwrap_err();
            assert!(err.contains(want), "{args}: got {err:?}, want {want:?}");
        }
    }

    #[test]
    fn formats() {
        assert!(format_ok(Format::Date, "2026-10-04") && !format_ok(Format::Date, "04/10/2026"));
        assert!(format_ok(Format::Email, "riya@example.com") && !format_ok(Format::Email, "riya"));
        assert!(format_ok(Format::Uri, "https://x.dev/a") && !format_ok(Format::Uri, "x.dev"));
    }
}
