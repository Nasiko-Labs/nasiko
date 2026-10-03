//! Arguments checked against the schema the model was shown.
//!
//! Stricter than plain JSON Schema in one way: a key the tool does not declare is an error even
//! when the schema leaves `additionalProperties` open. The compact form never tells the model
//! extra keys are allowed, so an undeclared key is an invention, not an extension.
//!
//! `format` is not checked. Native tool calling does not enforce it either, and a format checker
//! that disagreed with the provider's would reject calls the client would have accepted.

use serde_json::Value;

use crate::schema::{Fields, Node, Ty};

pub(crate) fn arguments(params: Option<&Fields>, args: &Value) -> Result<(), String> {
    match params {
        Some(fields) => object(fields, args, "arguments"),
        None => match args.as_object() {
            Some(map) if map.is_empty() => Ok(()),
            Some(_) => Err("the tool takes no arguments".into()),
            None => Err("arguments must be a JSON object".into()),
        },
    }
}

fn object(fields: &Fields, value: &Value, path: &str) -> Result<(), String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{path} must be an object"))?;
    for field in &fields.fields {
        match map.get(&field.key) {
            Some(value) => node(&field.node, value, &format!("{path}.{}", field.key))?,
            None if field.required => {
                return Err(format!("{path} is missing required `{}`", field.key));
            }
            None => {}
        }
    }
    match map
        .keys()
        .find(|key| !fields.fields.iter().any(|f| f.key == **key))
    {
        Some(key) => Err(format!("{path} has undeclared `{key}`")),
        None => Ok(()),
    }
}

fn node(node: &Node, value: &Value, path: &str) -> Result<(), String> {
    let expect = |ok: bool, what: &str| {
        if ok {
            Ok(())
        } else {
            Err(format!("{path} must be {what}"))
        }
    };
    match &node.ty {
        Ty::Str { .. } => expect(value.is_string(), "a string"),
        Ty::Int => expect(is_integer(value), "an integer"),
        Ty::Num => expect(value.is_number(), "a number"),
        Ty::Bool => expect(value.is_boolean(), "a boolean"),
        Ty::StrEnum(allowed) => expect(
            value
                .as_str()
                .is_some_and(|s| allowed.iter().any(|a| a == s)),
            &format!("one of {allowed:?}"),
        ),
        Ty::IntEnum(allowed) => expect(
            value.as_i64().is_some_and(|i| allowed.contains(&i)),
            &format!("one of {allowed:?}"),
        ),
        Ty::Array(items) => {
            let values = value
                .as_array()
                .ok_or_else(|| format!("{path} must be an array"))?;
            values
                .iter()
                .enumerate()
                .try_for_each(|(i, v)| self::node(items, v, &format!("{path}[{i}]")))
        }
        Ty::Object(fields) => object(fields, value, path),
        Ty::AnyObject => expect(value.is_object(), "an object"),
    }
}

/// JSON Schema's `integer`: any number with no fractional part, so `30.0` qualifies.
fn is_integer(value: &Value) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::compile;
    use crate::types::ToolDef;
    use serde_json::json;

    fn check(parameters: Option<Value>, args: Value) -> Result<(), String> {
        let tools = compile(&[ToolDef {
            name: "t".into(),
            description: None,
            parameters,
        }])
        .unwrap();
        arguments(tools[0].params.as_ref(), &args)
    }

    fn calendar() -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "owner": {
                    "type": "object",
                    "properties": {"id": {"type": "integer"}, "score": {"type": "number"}},
                    "required": ["id"]
                },
                "level": {"type": "integer", "enum": [1, 2]},
                "meta": {"type": "object"},
                "flag": {"type": "boolean"}
            },
            "required": ["title", "start"]
        }))
    }

    #[test]
    fn a_complete_valid_call_passes() {
        let args = json!({
            "title": "Retro", "start": "2026-10-04T10:00:00+05:30", "duration_min": 30,
            "attendees": ["a@example.com"], "visibility": "private",
            "owner": {"id": 1, "score": 0.5}, "level": 2, "meta": {"any": [1]}, "flag": true
        });
        assert_eq!(check(calendar(), args), Ok(()));
    }

    #[test]
    fn only_required_fields_are_needed() {
        assert_eq!(
            check(calendar(), json!({"title": "t", "start": "s"})),
            Ok(())
        );
    }

    #[test]
    fn every_kind_of_violation_is_caught() {
        let base = json!({"title": "t", "start": "s"});
        let with = |key: &str, value: Value| {
            let mut args = base.clone();
            args[key] = value;
            args
        };
        for (args, why) in [
            (json!({"start": "s"}), "missing required"),
            (json!([]), "not an object"),
            (json!("x"), "not an object"),
            (with("title", json!(5)), "wrong type for string"),
            (with("duration_min", json!("30")), "string for integer"),
            (with("duration_min", json!(30.5)), "fraction for integer"),
            (with("flag", json!("true")), "string for boolean"),
            (with("visibility", json!("secret")), "enum violation"),
            (
                with("visibility", json!("Public")),
                "enum is case sensitive",
            ),
            (with("level", json!(3)), "integer enum violation"),
            (
                with("attendees", json!("a@example.com")),
                "string for array",
            ),
            (with("attendees", json!(["a", 5])), "bad array item"),
            (
                with("owner", json!({"score": 1})),
                "nested missing required",
            ),
            (with("owner", json!({"id": 1, "x": 1})), "nested undeclared"),
            (with("meta", json!([])), "array for object"),
            (with("colour", json!("red")), "undeclared key"),
            (with("duration_min", Value::Null), "null for optional"),
        ] {
            assert!(check(calendar(), args.clone()).is_err(), "{why}: {args}");
        }
    }

    #[test]
    fn whole_number_floats_count_as_integers() {
        let args = json!({"title": "t", "start": "s", "duration_min": 30.0});
        assert_eq!(check(calendar(), args), Ok(()));
    }

    #[test]
    fn a_tool_with_no_parameters_accepts_only_an_empty_object() {
        assert_eq!(check(None, json!({})), Ok(()));
        assert!(check(None, json!({"x": 1})).is_err());
        assert!(check(None, json!(null)).is_err());
    }

    #[test]
    fn errors_name_the_path() {
        let args = json!({"title": "t", "start": "s", "attendees": ["a", 5]});
        let message = check(calendar(), args).unwrap_err();
        assert!(message.contains("arguments.attendees[1]"), "{message}");
    }
}
