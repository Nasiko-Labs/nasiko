//! Call arguments vs the typed schema. Strict and fail-closed: a value either matches exactly or
//! the call is rejected — no coercion, no case-folding, no nearest-enum guess.

use serde_json::{Map, Number, Value};

use crate::error::ArgError;
use crate::schema::{Format, Prop, Ty};

/// Validate a call's argument object against a tool's root properties.
pub(crate) fn validate_args(args: &Map<String, Value>, params: &[Prop]) -> Result<(), ArgError> {
    validate_object(args, params, "")
}

fn validate_object(obj: &Map<String, Value>, props: &[Prop], path: &str) -> Result<(), ArgError> {
    for p in props {
        let child = format!("{path}/{}", p.key);
        match obj.get(&p.key) {
            Some(v) => validate_prop(v, p, &child)?,
            None if p.required => return Err(ArgError::MissingRequired(child)),
            None => {}
        }
    }
    // Strict: a key the schema does not describe is a hallucinated argument, not a pass-through.
    if let Some(key) = obj.keys().find(|k| !props.iter().any(|p| &p.key == *k)) {
        return Err(ArgError::UnknownKey(format!("{path}/{key}")));
    }
    Ok(())
}

fn validate_prop(v: &Value, p: &Prop, path: &str) -> Result<(), ArgError> {
    validate_ty(v, &p.ty, path)?;
    if let Some(n) = v.as_f64() {
        let below = p
            .min
            .as_ref()
            .and_then(Number::as_f64)
            .is_some_and(|m| n < m);
        let above = p
            .max
            .as_ref()
            .and_then(Number::as_f64)
            .is_some_and(|m| n > m);
        if below || above {
            return Err(ArgError::OutOfRange { path: path.into() });
        }
    }
    Ok(())
}

fn wrong(path: &str, expected: &'static str) -> ArgError {
    ArgError::WrongType {
        path: path.into(),
        expected,
    }
}

fn validate_ty(v: &Value, ty: &Ty, path: &str) -> Result<(), ArgError> {
    match ty {
        Ty::Str(fmt) => {
            let s = v.as_str().ok_or_else(|| wrong(path, "string"))?;
            if let Some(f) = fmt
                && !format_ok(*f, s)
            {
                return Err(ArgError::BadFormat {
                    path: path.into(),
                    format: f.schema_name(),
                });
            }
            Ok(())
        }
        // `30.0` is not an integer here (stricter than JSON Schema): the call text is passed
        // through verbatim, so a typed tool would otherwise receive a float.
        Ty::Int if v.is_i64() || v.is_u64() => Ok(()),
        Ty::Int => Err(wrong(path, "integer")),
        Ty::Num if v.is_number() => Ok(()),
        Ty::Num => Err(wrong(path, "number")),
        Ty::Bool if v.is_boolean() => Ok(()),
        Ty::Bool => Err(wrong(path, "boolean")),
        Ty::Null if v.is_null() => Ok(()),
        Ty::Null => Err(wrong(path, "null")),
        Ty::Enum(values) if values.contains(v) => Ok(()),
        Ty::Enum(_) => Err(ArgError::NotInEnum { path: path.into() }),
        Ty::Array(items) => {
            let arr = v.as_array().ok_or_else(|| wrong(path, "array"))?;
            for (i, item) in arr.iter().enumerate() {
                validate_ty(item, items, &format!("{path}/{i}"))?;
            }
            Ok(())
        }
        Ty::Obj(props) => {
            let obj = v.as_object().ok_or_else(|| wrong(path, "object"))?;
            validate_object(obj, props, path)
        }
    }
}

// --------------------------------------------------------------------------
// Format shape checks — deterministic, dependency-free. `date-time`/`date`/`time`/`uuid` are
// checked structurally; `email`/`uri` are minimal shape checks (documented in the README).
// --------------------------------------------------------------------------

fn format_ok(f: Format, s: &str) -> bool {
    match f {
        Format::DateTime => is_datetime(s),
        Format::Date => is_date(s),
        Format::Time => is_time(s, false),
        Format::Email => is_email(s),
        Format::Uri => is_uri(s),
        Format::Uuid => is_uuid(s),
    }
}

fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, s.get(n..)?))
}

fn expect(s: &str, c: char) -> Option<&str> {
    s.strip_prefix(c)
}

/// `YYYY-MM-DD`, month 1–12, day valid for the month (leap years honoured).
fn is_date(s: &str) -> bool {
    parse_date(s).is_some_and(str::is_empty)
}

fn parse_date(s: &str) -> Option<&str> {
    let (y, s) = digits(s, 4)?;
    let (m, s) = digits(expect(s, '-')?, 2)?;
    let (d, s) = digits(expect(s, '-')?, 2)?;
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max_day = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=max_day).contains(&d).then_some(s)
}

/// `HH:MM:SS[.frac]` then, if `offset_required`, `Z` or `±HH:MM` (optional otherwise).
fn is_time(s: &str, offset_required: bool) -> bool {
    let Some(rest) = parse_partial_time(s) else {
        return false;
    };
    if rest.is_empty() {
        return !offset_required;
    }
    is_offset(rest)
}

fn parse_partial_time(s: &str) -> Option<&str> {
    let (h, s) = digits(s, 2)?;
    let (m, s) = digits(expect(s, ':')?, 2)?;
    let (sec, mut s) = digits(expect(s, ':')?, 2)?;
    if h > 23 || m > 59 || sec > 60 {
        return None;
    }
    if let Some(frac) = s.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = frac.get(n..)?;
    }
    Some(s)
}

fn is_offset(s: &str) -> bool {
    if s == "Z" || s == "z" {
        return true;
    }
    let Some(rest) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) else {
        return false;
    };
    let Some((h, rest)) = digits(rest, 2) else {
        return false;
    };
    let Some((m, rest)) = expect(rest, ':').and_then(|r| digits(r, 2)) else {
        return false;
    };
    rest.is_empty() && h <= 23 && m <= 59
}

/// RFC 3339 `date-time`: full date, `T`, full time with a mandatory offset.
fn is_datetime(s: &str) -> bool {
    let Some(rest) = parse_date(s) else {
        return false;
    };
    let Some(time) = rest.strip_prefix('T').or_else(|| rest.strip_prefix('t')) else {
        return false;
    };
    is_time(time, true)
}

fn is_email(s: &str) -> bool {
    let mut parts = s.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(char::is_whitespace)
}

fn is_uri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && !s.chars().any(char::is_whitespace)
}

fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_shapes() {
        for ok in [
            "2026-10-05T15:00:00+05:30",
            "2026-10-04T10:00:00Z",
            "2024-02-29T23:59:60.123-00:00",
        ] {
            assert!(is_datetime(ok), "{ok}");
        }
        for bad in [
            "2026-10-05",
            "2026-10-05T15:00:00",
            "2026-13-05T15:00:00Z",
            "2025-02-29T10:00:00Z",
            "2026-10-05 15:00:00Z",
            "2026-10-05T25:00:00Z",
            "Monday 3pm",
            "2026-10-05T15:00:00+5:30",
        ] {
            assert!(!is_datetime(bad), "{bad}");
        }
    }

    #[test]
    fn other_formats() {
        assert!(is_date("2026-10-03") && !is_date("2026-10-3"));
        assert!(
            is_time("10:00:00", false)
                && is_time("10:00:00+05:30", false)
                && !is_time("10:00", false)
        );
        assert!(
            is_email("riya@example.com") && !is_email("riya@example") && !is_email("a b@x.com")
        );
        assert!(
            is_uri("https://x.dev/a")
                && is_uri("mailto:a@b.c")
                && !is_uri("x.dev")
                && !is_uri("1a:x")
        );
        assert!(is_uuid("123e4567-e89b-12d3-a456-426614174000") && !is_uuid("123e4567"));
    }
}
