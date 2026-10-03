//! Argument validation against the lowered schema.
//!
//! Semantics follow JSON Schema for the supported subset: no coercion, no defaults, extras
//! allowed unless `additionalProperties: false`, `integer` means "a number with no fractional
//! part" (so `1.0` is an integer), `format` is an annotation and is not checked. Numeric
//! comparisons are exact: integers compare as `i128`, an integer against a float compares via the
//! float's floor, and only float-to-float comparisons use `f64`.

use std::cmp::Ordering;

use serde_json::{Number, Value};

use crate::schema::{Kind, Node, ObjectSchema, Scalar, ScalarBase};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Violation {
    pub path: String,
    pub reason: String,
}

fn violation(path: &str, reason: impl Into<String>) -> Violation {
    Violation {
        path: if path.is_empty() {
            "/".into()
        } else {
            path.into()
        },
        reason: reason.into(),
    }
}

/// Validate `value` against `node`; `path` is the JSON pointer of `value`.
pub(crate) fn validate(node: &Node, value: &Value, path: &str) -> Result<(), Violation> {
    match &node.kind {
        Kind::Any => Ok(()),
        Kind::Enum { members, .. } => {
            let hit = members.iter().any(|m| json_equal(m, value));
            if hit {
                Ok(())
            } else {
                Err(violation(
                    path,
                    "value is not one of the allowed enum members",
                ))
            }
        }
        _ if value.is_null() && node.nullable => Ok(()),
        Kind::Scalar(s) => validate_scalar(s, value, path),
        Kind::Array {
            items,
            min_items,
            max_items,
        } => {
            let Value::Array(elems) = value else {
                return Err(violation(path, "expected an array"));
            };
            if let Some(min) = min_items
                && (elems.len() as u64) < *min
            {
                return Err(violation(path, format!("expected at least {min} items")));
            }
            if let Some(max) = max_items
                && (elems.len() as u64) > *max
            {
                return Err(violation(path, format!("expected at most {max} items")));
            }
            if let Some(items) = items {
                for (i, elem) in elems.iter().enumerate() {
                    validate(items, elem, &format!("{path}/{i}"))?;
                }
            }
            Ok(())
        }
        Kind::Object(o) => validate_object(o, value, path),
    }
}

fn validate_object(o: &ObjectSchema, value: &Value, path: &str) -> Result<(), Violation> {
    let Value::Object(map) = value else {
        return Err(violation(path, "expected an object"));
    };
    if let Some(required) = &o.required {
        for name in required {
            if !map.contains_key(name) {
                return Err(violation(
                    path,
                    format!("missing required property `{name}`"),
                ));
            }
        }
    }
    let props = o.properties.as_deref().unwrap_or(&[]);
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort_unstable();
    for key in keys {
        let child_path = format!("{path}/{key}");
        match props.iter().find(|(n, _)| n == key) {
            Some((_, node)) => {
                if let Some(v) = map.get(key) {
                    validate(node, v, &child_path)?;
                }
            }
            None => {
                if o.additional == Some(false) {
                    return Err(violation(&child_path, "additional property not allowed"));
                }
            }
        }
    }
    Ok(())
}

fn validate_scalar(s: &Scalar, value: &Value, path: &str) -> Result<(), Violation> {
    match s.base {
        ScalarBase::Str => {
            let Value::String(text) = value else {
                return Err(violation(path, "expected a string"));
            };
            let len = text.chars().count() as u64;
            if let Some(min) = s.min_length
                && len < min
            {
                return Err(violation(
                    path,
                    format!("expected at least {min} characters"),
                ));
            }
            if let Some(max) = s.max_length
                && len > max
            {
                return Err(violation(
                    path,
                    format!("expected at most {max} characters"),
                ));
            }
            Ok(())
        }
        ScalarBase::Bool => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(violation(path, "expected a boolean")),
        },
        ScalarBase::Null => match value {
            Value::Null => Ok(()),
            _ => Err(violation(path, "expected null")),
        },
        ScalarBase::Int | ScalarBase::Num => {
            let Value::Number(n) = value else {
                return Err(violation(path, "expected a number"));
            };
            if s.base == ScalarBase::Int && !is_integral(n) {
                return Err(violation(path, "expected an integer"));
            }
            if let Some(min) = &s.minimum
                && cmp_numbers(n, min) == Ordering::Less
            {
                return Err(violation(path, format!("expected a value >= {min}")));
            }
            if let Some(min) = &s.exclusive_minimum
                && cmp_numbers(n, min) != Ordering::Greater
            {
                return Err(violation(path, format!("expected a value > {min}")));
            }
            if let Some(max) = &s.maximum
                && cmp_numbers(n, max) == Ordering::Greater
            {
                return Err(violation(path, format!("expected a value <= {max}")));
            }
            if let Some(max) = &s.exclusive_maximum
                && cmp_numbers(n, max) != Ordering::Less
            {
                return Err(violation(path, format!("expected a value < {max}")));
            }
            Ok(())
        }
    }
}

/// JSON Schema integer semantics: `1.0` is an integer, `1.5` is not.
pub(crate) fn is_integral(n: &Number) -> bool {
    if n.is_i64() || n.is_u64() {
        return true;
    }
    n.as_f64()
        .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

fn as_i128(n: &Number) -> Option<i128> {
    if let Some(i) = n.as_i64() {
        return Some(i128::from(i));
    }
    n.as_u64().map(i128::from)
}

/// Exact total order over JSON numbers (NaN cannot occur: JSON has no NaN literal).
pub(crate) fn cmp_numbers(a: &Number, b: &Number) -> Ordering {
    match (as_i128(a), as_i128(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(x), None) => cmp_int_float(x, b.as_f64().unwrap_or(0.0)),
        (None, Some(y)) => cmp_int_float(y, a.as_f64().unwrap_or(0.0)).reverse(),
        (None, None) => a
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&b.as_f64().unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
    }
}

/// Compare an integer with a float without converting the integer to `f64`.
fn cmp_int_float(i: i128, f: f64) -> Ordering {
    // Every i128 fits comfortably inside f64's range, so an out-of-range float is decisive.
    const I128_MAX_F: f64 = 1.7014118346046923e38;
    if f >= I128_MAX_F {
        return Ordering::Less;
    }
    if f <= -I128_MAX_F {
        return Ordering::Greater;
    }
    // `floor` of a finite in-range float is an exact integer representable as i128.
    let floor = f.floor();
    let floor_i = floor as i128;
    match i.cmp(&floor_i) {
        Ordering::Equal => {
            if f == floor {
                Ordering::Equal
            } else {
                // i == floor(f) < f
                Ordering::Less
            }
        }
        other => other,
    }
}

/// Equality used for enum membership: exact for strings, booleans and null; numeric for numbers.
pub(crate) fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => cmp_numbers(x, y) == Ordering::Equal,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn num(v: Value) -> Number {
        match v {
            Value::Number(n) => n,
            _ => panic!("not a number"),
        }
    }

    #[test]
    fn integer_semantics_follow_json_schema() {
        assert!(is_integral(&num(json!(1))));
        assert!(is_integral(&num(json!(1.0))));
        assert!(is_integral(&num(json!(-0.0))));
        assert!(!is_integral(&num(json!(1.5))));
    }

    #[test]
    fn comparisons_are_exact_around_two_to_the_fifty_three() {
        let big = num(json!(9007199254740993_i64)); // 2^53 + 1
        let float = num(json!(9007199254740992.0)); // 2^53
        assert_eq!(cmp_numbers(&big, &float), Ordering::Greater);
        assert_eq!(cmp_numbers(&float, &big), Ordering::Less);
        assert!(!json_equal(
            &json!(9007199254740993_i64),
            &json!(9007199254740992.0)
        ));
        assert!(json_equal(&json!(1), &json!(1.0)));
        assert_eq!(
            cmp_numbers(&num(json!(u64::MAX)), &num(json!(i64::MIN))),
            Ordering::Greater
        );
        assert_eq!(
            cmp_numbers(&num(json!(2)), &num(json!(2.5))),
            Ordering::Less
        );
        assert_eq!(
            cmp_numbers(&num(json!(3)), &num(json!(2.5))),
            Ordering::Greater
        );
        assert_eq!(
            cmp_numbers(&num(json!(-3)), &num(json!(-2.5))),
            Ordering::Less
        );
        assert_eq!(
            cmp_numbers(&num(json!(0)), &num(json!(1e300))),
            Ordering::Less
        );
        assert_eq!(
            cmp_numbers(&num(json!(0)), &num(json!(-1e300))),
            Ordering::Greater
        );
    }
}
