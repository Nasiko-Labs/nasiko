//! Fail-closed validation of call arguments against a tool's schema.
//!
//! Semantics follow JSON Schema for every keyword this crate understands, with one deliberate
//! tightening: an object that declares `properties` rejects keys it does not declare unless
//! `additionalProperties` allows them. The model was shown only the declared keys, so anything
//! else is a hallucination — the same policy OpenAI applies in strict mode. A keyword this crate
//! cannot enforce makes the call invalid rather than unchecked.

use serde_json::{Map, Value};

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
        && !values.contains(value)
    {
        return Err(violation(path, ArgumentFault::NotInEnum));
    }
    if let Some(expected) = &node.const_value
        && expected != value
    {
        return Err(violation(path, ArgumentFault::ConstMismatch));
    }
    check_bounds(&node.bounds, value, path)?;
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
            if limit(&b.multiple_of).is_some_and(|m| !is_multiple(x, m)) {
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
            if b.unique_items == Some(true)
                && items
                    .iter()
                    .enumerate()
                    .any(|(i, a)| items.iter().skip(i + 1).any(|b| a == b))
            {
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

/// Tolerant of binary floating point (`0.3` is a multiple of `0.1`).
fn is_multiple(x: f64, step: f64) -> bool {
    if step <= 0.0 {
        return false;
    }
    let quotient = x / step;
    (quotient - quotient.round()).abs() <= 1e-9 * quotient.abs().max(1.0)
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
    fn format_is_an_annotation_not_an_assertion() {
        let schema = one_prop(json!({"type": "string", "format": "date-time"}));
        assert_eq!(fault(schema, json!({"v": "next Tuesday"})), None);
    }

    #[test]
    fn an_unenforceable_keyword_fails_closed() {
        let schema = one_prop(json!({"type": "string", "not": {"const": "x"}}));
        assert_eq!(
            fault(schema, json!({"v": "y"})),
            Some((
                "/v".into(),
                ArgumentFault::Unvalidatable {
                    keyword: "not".into()
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
