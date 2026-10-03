//! Fail-closed validation of call arguments against a tool's schema.
//!
//! Semantics follow JSON Schema for every keyword this crate understands, with one deliberate
//! tightening: an object that declares `properties` rejects keys it does not declare unless
//! `additionalProperties` allows them. The model was shown only the declared keys, so anything
//! else is a hallucination — the same policy OpenAI applies in strict mode. A keyword this crate
//! cannot enforce makes the call invalid rather than unchecked.

use serde_json::{Map, Number, Value};

use crate::error::{ArgumentFault, CompactError};
use crate::schema::{Additional, Bounds, Kind, Node, ObjectShape};

/// Validate a call's arguments (`value`, normally an object) against the tool's root schema.
pub(crate) fn validate(tool: &str, root: &Node, value: &Value) -> Result<(), CompactError> {
    check(root, value, "").map_err(|(path, fault)| CompactError::InvalidArguments {
        tool: tool.to_string(),
        path,
        fault,
    })
}

type Violation = (String, ArgumentFault);

fn check(node: &Node, value: &Value, path: &str) -> Result<(), Violation> {
    if let Some(keyword) = &node.unvalidatable {
        return Err(violation(
            path,
            ArgumentFault::Unvalidatable {
                keyword: keyword.clone(),
            },
        ));
    }
    if !node.types.is_empty() && !node.types.iter().any(|k| kind_matches(*k, value)) {
        let expected = node
            .types
            .iter()
            .map(|k| k.keyword())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(violation(path, ArgumentFault::WrongType { expected }));
    }
    if let Some(values) = &node.enum_values
        && !values.iter().any(|v| json_equal(v, value))
    {
        return Err(violation(path, ArgumentFault::NotInEnum));
    }
    if let Some(expected) = &node.const_value
        && !json_equal(expected, value)
    {
        return Err(violation(path, ArgumentFault::ConstMismatch));
    }
    check_bounds(&node.bounds, value, path)?;
    check_format(node.format.as_deref(), value, path)?;
    match value {
        Value::Array(items) => {
            if let Some(item) = &node.items {
                for (i, element) in items.iter().enumerate() {
                    check(item, element, &format!("{path}/{i}"))?;
                }
            }
        }
        Value::Object(map) => {
            if let Some(shape) = &node.object {
                check_object(shape, map, path)?;
            }
        }
        _ => {}
    }
    check_assertions(node, value, path)?;
    check_combinators(node, value, path)
}

fn violation(path: &str, fault: ArgumentFault) -> Violation {
    (path.to_string(), fault)
}

fn kind_matches(kind: Kind, value: &Value) -> bool {
    match kind {
        Kind::String => value.is_string(),
        Kind::Number => value.is_number(),
        // JSON Schema: an integer is any number with a zero fractional part, so `30.0` counts.
        // It is passed through exactly as written.
        Kind::Integer => match value {
            Value::Number(n) => {
                n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0)
            }
            _ => false,
        },
        Kind::Boolean => value.is_boolean(),
        Kind::Null => value.is_null(),
        Kind::Object => value.is_object(),
        Kind::Array => value.is_array(),
    }
}

/// `date-time`, `date` and `time` are checked for ISO 8601 / RFC 3339 shape, so "next Monday"
/// can never pass as a timestamp. The offset is optional (a model that omits it wrote a valid
/// local time, and callers decide how to read it). Every other format is an annotation, as JSON
/// Schema specifies.
fn check_format(format: Option<&str>, value: &Value, path: &str) -> Result<(), Violation> {
    let (Some(format), Value::String(s)) = (format, value) else {
        return Ok(());
    };
    let valid = match format {
        "date-time" => is_date_time(s),
        "date" => is_date(s),
        "time" => is_time(s),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(violation(
            path,
            ArgumentFault::Constraint { keyword: "format" },
        ))
    }
}

fn digits(s: &str, from: usize, len: usize) -> Option<u32> {
    let part = s.get(from..from + len)?;
    part.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| part.parse().ok())
        .flatten()
}

fn is_date(s: &str) -> bool {
    s.len() == 10
        && s.as_bytes().get(4) == Some(&b'-')
        && s.as_bytes().get(7) == Some(&b'-')
        && digits(s, 0, 4).is_some()
        && digits(s, 5, 2).is_some_and(|m| (1..=12).contains(&m))
        && digits(s, 8, 2).is_some_and(|d| (1..=31).contains(&d))
}

/// `HH:MM[:SS[.fraction]][Z|±HH:MM]`.
fn is_time(s: &str) -> bool {
    let hour_minute = s.as_bytes().get(2) == Some(&b':')
        && digits(s, 0, 2).is_some_and(|h| h <= 23)
        && digits(s, 3, 2).is_some_and(|m| m <= 59);
    if !hour_minute {
        return false;
    }
    let mut rest = s.get(5..).unwrap_or_default();
    if let Some(after) = rest.strip_prefix(':') {
        if digits(after, 0, 2).is_none_or(|sec| sec > 60) {
            return false;
        }
        rest = after.get(2..).unwrap_or_default();
        if let Some(fraction) = rest.strip_prefix('.') {
            let len = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if len == 0 {
                return false;
            }
            rest = fraction.get(len..).unwrap_or_default();
        }
    }
    match rest {
        "" | "Z" | "z" => true,
        offset => {
            matches!(offset.as_bytes().first(), Some(b'+' | b'-'))
                && offset.len() == 6
                && offset.as_bytes().get(3) == Some(&b':')
                && digits(offset, 1, 2).is_some_and(|h| h <= 23)
                && digits(offset, 4, 2).is_some_and(|m| m <= 59)
        }
    }
}

fn is_date_time(s: &str) -> bool {
    let separator = s.as_bytes().get(10);
    matches!(separator, Some(b'T' | b't' | b' '))
        && s.get(..10).is_some_and(is_date)
        && s.get(11..).is_some_and(is_time)
}

fn check_bounds(b: &Bounds, value: &Value, path: &str) -> Result<(), Violation> {
    let fail = |keyword: &'static str| Err(violation(path, ArgumentFault::Constraint { keyword }));
    match value {
        Value::Number(n) => {
            let Some(x) = n.as_f64() else {
                return Ok(());
            };
            let limit =
                |bound: &Option<serde_json::Number>| bound.as_ref().and_then(|v| v.as_f64());
            if limit(&b.minimum).is_some_and(|m| x < m) {
                return fail("minimum");
            }
            if limit(&b.maximum).is_some_and(|m| x > m) {
                return fail("maximum");
            }
            if limit(&b.exclusive_minimum).is_some_and(|m| x <= m) {
                return fail("exclusiveMinimum");
            }
            if limit(&b.exclusive_maximum).is_some_and(|m| x >= m) {
                return fail("exclusiveMaximum");
            }
            if b.multiple_of.as_ref().is_some_and(|m| !is_multiple(n, m)) {
                return fail("multipleOf");
            }
        }
        Value::String(s) => {
            let length = s.chars().count() as u64;
            if b.min_length.is_some_and(|m| length < m) {
                return fail("minLength");
            }
            if b.max_length.is_some_and(|m| length > m) {
                return fail("maxLength");
            }
            if b.pattern.as_ref().is_some_and(|p| !p.regex.is_match(s)) {
                return fail("pattern");
            }
        }
        Value::Array(items) => {
            let count = items.len() as u64;
            if b.min_items.is_some_and(|m| count < m) {
                return fail("minItems");
            }
            if b.max_items.is_some_and(|m| count > m) {
                return fail("maxItems");
            }
            if b.unique_items == Some(true) && has_duplicates(items) {
                return fail("uniqueItems");
            }
        }
        Value::Object(map) => {
            let count = map.len() as u64;
            if b.min_properties.is_some_and(|m| count < m) {
                return fail("minProperties");
            }
            if b.max_properties.is_some_and(|m| count > m) {
                return fail("maxProperties");
            }
        }
        _ => {}
    }
    Ok(())
}

/// Exact for integers; for fractions, tolerant of binary floating point (`0.3` is a multiple of
/// `0.1`) with a fixed tolerance on the remainder, never one that grows with the value.
fn is_multiple(value: &Number, step: &Number) -> bool {
    if let (Some(v), Some(s)) = (as_i128(value), as_i128(step)) {
        return s != 0 && v % s == 0;
    }
    let (Some(v), Some(s)) = (value.as_f64(), step.as_f64()) else {
        return false;
    };
    if s <= 0.0 {
        return false;
    }
    let remainder = (v / s).fract().abs();
    !(1e-9..=1.0 - 1e-9).contains(&remainder)
}

fn as_i128(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}

/// JSON Schema equality: numbers compare by value, so `1.0` equals `1`.
fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (as_i128(x), as_i128(y)) {
            (Some(x), Some(y)) => x == y,
            _ => x.as_f64() == y.as_f64(),
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_equal(a, b))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_equal(v, w)))
        }
        _ => a == b,
    }
}

/// `uniqueItems` in O(n log n): sort a value-normalised rendering of each item.
fn has_duplicates(items: &[Value]) -> bool {
    let mut keys: Vec<String> = items.iter().map(|v| normalized(v).to_string()).collect();
    keys.sort_unstable();
    keys.windows(2).any(|pair| pair[0] == pair[1])
}

/// Integral floats rewritten as integers, so `1.0` and `1` render the same.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Number(n) if as_i128(n).is_none() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Value::from(f as i64),
            _ => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(normalized).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), normalized(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn check_object(
    shape: &ObjectShape,
    map: &Map<String, Value>,
    path: &str,
) -> Result<(), Violation> {
    for name in shape.required.iter().flatten() {
        if !map.contains_key(name) {
            return Err(violation(
                &format!("{path}/{name}"),
                ArgumentFault::MissingRequired,
            ));
        }
    }
    for (key, value) in map {
        let key_path = format!("{path}/{key}");
        let declared = shape
            .properties
            .as_ref()
            .and_then(|props| props.iter().find(|p| &p.name == key));
        match (declared, &shape.additional) {
            (Some(prop), _) => check(&prop.node, value, &key_path)?,
            (None, Additional::Schema(schema)) => check(schema, value, &key_path)?,
            (None, Additional::Allowed(true)) => {}
            (None, Additional::Allowed(false)) => {
                return Err(violation(&key_path, ArgumentFault::Undeclared));
            }
            (None, Additional::Unspecified) => {
                if shape.properties.is_some() {
                    return Err(violation(&key_path, ArgumentFault::Undeclared));
                }
            }
        }
    }
    Ok(())
}

/// `not`, `if`/`then`/`else`, `dependentRequired`, `propertyNames` and `contains`.
fn check_assertions(node: &Node, value: &Value, path: &str) -> Result<(), Violation> {
    let fail = |keyword: &'static str| Err(violation(path, ArgumentFault::Constraint { keyword }));
    if let Some(not) = &node.not
        && check(not, value, path).is_ok()
    {
        return fail("not");
    }
    if let Some(cond) = &node.if_then_else
        && let Some(condition) = &cond.condition
    {
        let branch = if check(condition, value, path).is_ok() {
            &cond.then_branch
        } else {
            &cond.else_branch
        };
        if let Some(branch) = branch {
            check(branch, value, path)?;
        }
    }
    if let Value::Object(map) = value {
        for (key, names) in node.dependent_required.iter().flatten() {
            if !map.contains_key(key) {
                continue;
            }
            if let Some(missing) = names.iter().find(|n| !map.contains_key(*n)) {
                return Err(violation(
                    &format!("{path}/{missing}"),
                    ArgumentFault::MissingRequired,
                ));
            }
        }
        if let Some(names) = &node.property_names {
            for key in map.keys() {
                if check(names, &Value::String(key.clone()), path).is_err() {
                    return Err(violation(
                        &format!("{path}/{key}"),
                        ArgumentFault::Constraint {
                            keyword: "propertyNames",
                        },
                    ));
                }
            }
        }
    }
    if let (Value::Array(items), Some(contains)) = (value, &node.contains) {
        let matching = contains.schema.as_ref().map_or(items.len(), |schema| {
            items
                .iter()
                .filter(|item| check(schema, item, path).is_ok())
                .count()
        }) as u64;
        if matching < contains.min.unwrap_or(1) {
            return fail("contains");
        }
        if contains.max.is_some_and(|max| matching > max) {
            return fail("maxContains");
        }
    }
    Ok(())
}

fn check_combinators(node: &Node, value: &Value, path: &str) -> Result<(), Violation> {
    for branch in node.all_of.iter().flatten() {
        check(branch, value, path)?;
    }
    if let Some(branches) = &node.any_of
        && !branches.iter().any(|b| check(b, value, path).is_ok())
    {
        return Err(violation(path, ArgumentFault::NoAlternative));
    }
    if let Some(branches) = &node.one_of {
        let matching = branches
            .iter()
            .filter(|b| check(b, value, path).is_ok())
            .count();
        match matching {
            0 => return Err(violation(path, ArgumentFault::NoAlternative)),
            1 => {}
            _ => return Err(violation(path, ArgumentFault::AmbiguousAlternative)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ToolDef;
    use crate::schema::analyze;
    use serde_json::json;

    fn root(params: Value) -> Node {
        analyze(&ToolDef::new("t", None, Some(params))).root
    }

    fn fault(params: Value, args: Value) -> Option<(String, ArgumentFault)> {
        match validate("t", &root(params), &args) {
            Ok(()) => None,
            Err(CompactError::InvalidArguments { path, fault, .. }) => Some((path, fault)),
            Err(other) => panic!("unexpected {other:?}"),
        }
    }

    fn one_prop(schema: Value) -> Value {
        json!({"type": "object", "properties": {"v": schema}})
    }

    #[test]
    fn type_table() {
        let cases = [
            (json!({"type": "string"}), json!("x"), true),
            (json!({"type": "string"}), json!(1), false),
            (json!({"type": "integer"}), json!(3), true),
            (json!({"type": "integer"}), json!(1.5), false),
            (json!({"type": "integer"}), json!(30.0), true),
            (json!({"type": "number"}), json!(1.5), true),
            (json!({"type": "boolean"}), json!("true"), false),
            (json!({"type": "array"}), json!("x"), false),
            (json!({"type": "string"}), json!(null), false),
            (json!({"type": ["string", "null"]}), json!(null), true),
            (json!({}), json!({"anything": [1]}), true),
        ];
        for (schema, value, ok) in cases {
            let result = fault(one_prop(schema.clone()), json!({ "v": value }));
            assert_eq!(result.is_none(), ok, "{schema} vs {value}: {result:?}");
        }
    }

    #[test]
    fn enum_violations_are_rejected_never_repaired() {
        let schema = one_prop(json!({"type": "string", "enum": ["public", "private"]}));
        for bad in ["secret", "Private", " private"] {
            assert_eq!(
                fault(schema.clone(), json!({ "v": bad })),
                Some(("/v".into(), ArgumentFault::NotInEnum))
            );
        }
        assert_eq!(fault(schema, json!({"v": "private"})), None);
    }

    #[test]
    fn missing_required_is_rejected_with_its_path() {
        let schema = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
            "required": ["a"]
        });
        assert_eq!(
            fault(schema, json!({"b": "x"})),
            Some(("/a".into(), ArgumentFault::MissingRequired))
        );
    }

    #[test]
    fn undeclared_keys_are_rejected_only_when_properties_are_declared() {
        let declared = json!({"type": "object", "properties": {"a": {"type": "string"}}});
        assert_eq!(
            fault(declared, json!({"a": "x", "extra": 1})),
            Some(("/extra".into(), ArgumentFault::Undeclared))
        );
        let free_form = one_prop(json!({"type": "object"}));
        assert_eq!(fault(free_form, json!({"v": {"any": 1, "keys": 2}})), None);
        let open = json!({"type": "object", "properties": {}, "additionalProperties": true});
        assert_eq!(fault(open, json!({"x": 1})), None);
        let typed_extra = json!({"type": "object", "additionalProperties": {"type": "integer"}});
        assert_eq!(
            fault(typed_extra, json!({"x": "no"})),
            Some((
                "/x".into(),
                ArgumentFault::WrongType {
                    expected: "integer".into()
                }
            ))
        );
    }

    #[test]
    fn bounds_are_enforced() {
        let cases = [
            (
                json!({"type": "integer", "minimum": 1}),
                json!(0),
                "minimum",
            ),
            (
                json!({"type": "integer", "maximum": 5}),
                json!(6),
                "maximum",
            ),
            (
                json!({"type": "number", "exclusiveMinimum": 0}),
                json!(0),
                "exclusiveMinimum",
            ),
            (
                json!({"type": "number", "exclusiveMaximum": 1}),
                json!(1),
                "exclusiveMaximum",
            ),
            (
                json!({"type": "number", "multipleOf": 0.5}),
                json!(0.7),
                "multipleOf",
            ),
            (
                json!({"type": "string", "minLength": 2}),
                json!("é"),
                "minLength",
            ),
            (
                json!({"type": "string", "maxLength": 1}),
                json!("ab"),
                "maxLength",
            ),
            (
                json!({"type": "string", "pattern": "^[A-Z]+$"}),
                json!("ab"),
                "pattern",
            ),
            (
                json!({"type": "array", "minItems": 1}),
                json!([]),
                "minItems",
            ),
            (
                json!({"type": "array", "maxItems": 1}),
                json!([1, 2]),
                "maxItems",
            ),
            (
                json!({"type": "array", "uniqueItems": true}),
                json!([1, 1]),
                "uniqueItems",
            ),
        ];
        for (schema, value, keyword) in cases {
            assert_eq!(
                fault(one_prop(schema.clone()), json!({ "v": value })),
                Some(("/v".into(), ArgumentFault::Constraint { keyword })),
                "{schema}"
            );
        }
        assert_eq!(
            fault(
                one_prop(json!({"type": "number", "multipleOf": 0.1})),
                json!({"v": 0.3})
            ),
            None,
            "floating point multiples are tolerated"
        );
    }

    #[test]
    fn combinators_and_refs_are_enforced() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"anyOf": [{"type": "string"}, {"type": "integer"}]},
                "o": {"oneOf": [{"type": "integer"}, {"type": "number"}]},
                "r": {"$ref": "#/$defs/Pos"}
            },
            "$defs": {"Pos": {"type": "integer", "minimum": 0}}
        });
        assert_eq!(
            fault(schema.clone(), json!({"a": "x", "o": 1.5, "r": 3})),
            None
        );
        assert_eq!(
            fault(schema.clone(), json!({"a": true})),
            Some(("/a".into(), ArgumentFault::NoAlternative))
        );
        assert_eq!(
            fault(schema.clone(), json!({"o": 2})),
            Some(("/o".into(), ArgumentFault::AmbiguousAlternative))
        );
        assert_eq!(
            fault(schema, json!({"r": -1})),
            Some((
                "/r".into(),
                ArgumentFault::Constraint { keyword: "minimum" }
            ))
        );
    }

    #[test]
    fn date_time_formats_are_checked_for_shape_with_an_optional_offset() {
        let schema = one_prop(json!({"type": "string", "format": "date-time"}));
        for ok in [
            "2026-10-05T15:00:00+05:30",
            "2026-10-05T15:00:00Z",
            "2026-10-05T15:00:00.250-07:00",
            "2026-10-05T15:00",
            "2026-10-05 15:00:00",
        ] {
            assert_eq!(fault(schema.clone(), json!({ "v": ok })), None, "{ok}");
        }
        for bad in [
            "next Tuesday",
            "2026-13-05T15:00:00",
            "2026-10-05",
            "15:00",
            "2026-10-05T25:00",
        ] {
            assert_eq!(
                fault(schema.clone(), json!({ "v": bad })),
                Some(("/v".into(), ArgumentFault::Constraint { keyword: "format" })),
                "{bad}"
            );
        }
        let date = one_prop(json!({"type": "string", "format": "date"}));
        assert_eq!(fault(date.clone(), json!({"v": "2026-10-04"})), None);
        assert!(fault(date, json!({"v": "Oct 4"})).is_some());
    }

    #[test]
    fn other_formats_are_annotations() {
        let schema = one_prop(json!({"type": "string", "format": "email"}));
        assert_eq!(fault(schema, json!({"v": "not an email"})), None);
    }

    #[test]
    fn an_unenforceable_keyword_fails_closed() {
        let schema = one_prop(json!({
            "type": "object",
            "patternProperties": {"^x": {"type": "string"}}
        }));
        assert_eq!(
            fault(schema, json!({"v": {"xa": "y"}})),
            Some((
                "/v".into(),
                ArgumentFault::Unvalidatable {
                    keyword: "patternProperties".into()
                }
            ))
        );
    }

    #[test]
    fn nested_paths_point_at_the_offending_value() {
        let schema = json!({"type": "object", "properties": {
            "rows": {"type": "array", "items": {"type": "object",
                "properties": {"id": {"type": "integer"}}, "required": ["id"]}}
        }});
        assert_eq!(
            fault(schema, json!({"rows": [{"id": 1}, {"id": "two"}]})),
            Some((
                "/rows/1/id".into(),
                ArgumentFault::WrongType {
                    expected: "integer".into()
                }
            ))
        );
    }
}
