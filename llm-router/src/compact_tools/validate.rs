//! Schema validation for decoded tool arguments.
//!
//! Enforces `type`, `enum`, `const`, `required`, recursive `properties`, `items`,
//! `additionalProperties`, and `allOf`/`anyOf`/`oneOf`. Unsupported validation
//! keywords and malformed schema entries fail validation instead of being skipped.
//! Only annotation keywords are ignored.

use serde_json::{Map, Value};

const SUPPORTED_KEYWORDS: &[&str] = &[
    "type",
    "enum",
    "const",
    "required",
    "properties",
    "items",
    "additionalProperties",
    "allOf",
    "anyOf",
    "oneOf",
];

const ANNOTATION_KEYWORDS: &[&str] = &[
    "title",
    "description",
    "default",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$comment",
];

/// JSON type keywords this validator understands.
const KNOWN_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "null", "object", "array",
];

/// Recursion guard: schemas come from the request, but a pathologically nested one
/// must fail validation instead of overflowing the stack.
const MAX_DEPTH: usize = 128;

/// Validate `args` against a tool's `parameters` schema. `Ok(())` when the schema
/// places no constraints or the arguments satisfy all of them. `Err` carries a
/// deterministic, human-readable reason.
pub(super) fn validate_arguments(schema: Option<&Value>, args: &Value) -> Result<(), String> {
    let Some(schema) = schema.filter(|schema| !schema.is_null()) else {
        return Ok(());
    };
    validate_schema(schema, "", 0)?;
    check(schema, args, "", 0)
}

/// Validate schema structure even for optional fields or empty arrays that are absent
/// from the model output. Otherwise an unsupported constraint could be skipped simply
/// because the value it applies to was not provided.
fn validate_schema(schema: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "{}: schema nesting exceeds the maximum depth",
            describe(path)
        ));
    }
    let Some(map) = schema.as_object() else {
        return match schema {
            Value::Bool(_) => Ok(()),
            _ => Err(format!(
                "{}: schema must be an object or boolean",
                describe(path)
            )),
        };
    };

    for keyword in map.keys() {
        if !SUPPORTED_KEYWORDS.contains(&keyword.as_str())
            && !ANNOTATION_KEYWORDS.contains(&keyword.as_str())
        {
            return Err(format!(
                "{}: schema uses the unsupported keyword '{keyword}'",
                describe(path)
            ));
        }
    }
    if let Some(t) = map.get("type") {
        type_names(t, path)?;
    }
    if let Some(enum_value) = map.get("enum") {
        if !enum_value.is_array() {
            return Err(format!(
                "{}: schema 'enum' must be an array",
                describe(path)
            ));
        }
    }
    if let Some(required_value) = map.get("required") {
        let Some(required) = required_value.as_array() else {
            return Err(format!(
                "{}: schema 'required' must be an array",
                describe(path)
            ));
        };
        if required.iter().any(|name| !name.is_string()) {
            return Err(format!(
                "{}: schema 'required' entries must be strings",
                describe(path)
            ));
        }
    }
    if let Some(properties) = map.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(format!(
                "{}: schema 'properties' must be an object",
                describe(path)
            ));
        };
        for (name, sub) in properties {
            validate_schema(sub, &field(path, name), depth + 1)?;
        }
    }
    if let Some(items) = map.get("items") {
        if items.is_array() {
            return Err(format!(
                "{}: schema uses the unsupported tuple-style 'items'",
                describe(path)
            ));
        }
        validate_schema(items, path, depth + 1)?;
    }
    if let Some(additional) = map.get("additionalProperties") {
        match additional {
            Value::Bool(_) => {}
            Value::Object(_) => validate_schema(additional, path, depth + 1)?,
            _ => {
                return Err(format!(
                    "{}: 'additionalProperties' must be an object or boolean",
                    describe(path)
                ));
            }
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(value) = map.get(keyword) {
            let Some(schemas) = value.as_array() else {
                return Err(format!(
                    "{}: schema '{keyword}' must be an array",
                    describe(path)
                ));
            };
            for sub in schemas {
                validate_schema(sub, path, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// Validate one `value` against `schema`. `path` locates the value inside the
/// arguments for error messages ("" = the arguments object itself).
fn check(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!(
            "{}: schema nesting exceeds the maximum depth",
            describe(path)
        ));
    }
    let Some(map) = schema.as_object() else {
        return match schema {
            Value::Bool(true) => Ok(()),
            Value::Bool(false) => Err(format!("{}: the schema rejects any value", describe(path))),
            _ => Err(format!(
                "{}: schema must be an object or boolean",
                describe(path)
            )),
        };
    };

    for keyword in map.keys() {
        if !SUPPORTED_KEYWORDS.contains(&keyword.as_str())
            && !ANNOTATION_KEYWORDS.contains(&keyword.as_str())
        {
            return Err(format!(
                "{}: schema uses the unsupported keyword '{keyword}'",
                describe(path)
            ));
        }
    }

    if let Some(t) = map.get("type") {
        check_type(t, value, path)?;
    }
    match value {
        Value::Object(_) => check_object(map, value, path, depth)?,
        Value::Array(_) => check_array(map, value, path, depth)?,
        _ => {}
    }
    for combinator in ["allOf", "anyOf", "oneOf"] {
        check_combinator(combinator, map, value, path, depth)?;
    }
    check_enum(map, value, path)?;
    check_const(map, value, path)?;
    Ok(())
}

/// Check the `type` keyword: a single type name or an array of them.
fn check_type(t: &Value, value: &Value, path: &str) -> Result<(), String> {
    let names = type_names(t, path)?;
    if !names.iter().any(|name| type_matches(name, value)) {
        return Err(format!(
            "{}: expected {}, got {}",
            describe(path),
            names.join(" or "),
            json_type(value)
        ));
    }
    Ok(())
}

fn type_names<'a>(t: &'a Value, path: &str) -> Result<Vec<&'a str>, String> {
    let names: Vec<&str> = match t {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                let Some(s) = item.as_str() else {
                    return Err(format!(
                        "{}: schema has a non-string entry in its 'type' array",
                        describe(path)
                    ));
                };
                names.push(s);
            }
            names
        }
        _ => {
            return Err(format!(
                "{}: schema has an unsupported 'type' keyword",
                describe(path)
            ));
        }
    };
    for name in &names {
        if !KNOWN_TYPES.contains(name) {
            return Err(format!(
                "{}: schema uses the unsupported type '{name}'",
                describe(path)
            ));
        }
    }
    if names.is_empty() {
        // An empty type list matches nothing.
        return Err(format!(
            "{}: schema 'type' list matches no type",
            describe(path)
        ));
    }
    Ok(names)
}

/// Object rules: `required`, `properties`, `additionalProperties`.
fn check_object(
    schema: &Map<String, Value>,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    let Some(obj) = value.as_object() else {
        // Structural checks apply to objects only; `type` already rejected mismatches.
        return Ok(());
    };

    // Required first: a missing field is the most fundamental failure.
    if let Some(required_value) = schema.get("required") {
        let Some(required) = required_value.as_array() else {
            return Err(format!(
                "{}: schema 'required' must be an array",
                describe(path)
            ));
        };
        for name in required {
            let Some(name) = name.as_str() else {
                return Err(format!(
                    "{}: schema 'required' entries must be strings",
                    describe(path)
                ));
            };
            if !obj.contains_key(name) {
                return Err(format!(
                    "{prefix}missing required field '{name}'",
                    prefix = path_prefix(path)
                ));
            }
        }
    }

    let properties = match schema.get("properties") {
        None | Some(Value::Null) => None,
        Some(props @ Value::Object(_)) => Some(props),
        Some(_) => {
            return Err(format!(
                "{}: schema 'properties' must be an object",
                describe(path)
            ));
        }
    };

    for (key, item) in obj {
        match properties.and_then(|props| props.get(key)) {
            Some(sub) => check(sub, item, &field(path, key), depth + 1)?,
            None => match schema.get("additionalProperties") {
                Some(Value::Bool(false)) => {
                    return Err(format!(
                        "{prefix}unexpected field '{key}'",
                        prefix = path_prefix(path)
                    ));
                }
                Some(Value::Bool(true)) | None => {}
                Some(sub) => check(sub, item, &field(path, key), depth + 1)?,
            },
        }
    }
    Ok(())
}

/// Array rules: every element against `items`.
fn check_array(
    schema: &Map<String, Value>,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    let Some(items) = value.as_array() else {
        return Ok(());
    };
    match schema.get("items") {
        None | Some(Value::Bool(true)) => Ok(()),
        Some(item_schema) if item_schema.is_array() => Err(format!(
            "{}: schema uses the unsupported tuple-style 'items'",
            describe(path)
        )),
        Some(item_schema) => {
            for (index, item) in items.iter().enumerate() {
                check(item_schema, item, &element(path, index), depth + 1)?;
            }
            Ok(())
        }
    }
}

/// `allOf` (all must match), `anyOf` (at least one), `oneOf` (exactly one).
fn check_combinator(
    keyword: &str,
    schema: &Map<String, Value>,
    value: &Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    let Some(combinator_schema) = schema.get(keyword) else {
        return Ok(());
    };
    let Some(subschemas) = combinator_schema.as_array() else {
        return Err(format!(
            "{}: schema '{keyword}' must be an array",
            describe(path)
        ));
    };
    let mut matches = 0usize;
    let mut first_error = None;
    for sub in subschemas {
        match check(sub, value, path, depth + 1) {
            Ok(()) => matches += 1,
            Err(e) if first_error.is_none() => first_error = Some(e),
            Err(_) => {}
        }
    }
    let satisfied = match keyword {
        "allOf" => matches == subschemas.len(),
        "anyOf" => matches >= 1,
        _ => matches == 1, // oneOf
    };
    if satisfied {
        return Ok(());
    }
    Err(match (keyword, first_error) {
        ("allOf", Some(e)) => format!(
            "{}: does not satisfy all of the allOf schemas ({e})",
            describe(path)
        ),
        ("allOf", None) => format!(
            "{}: does not satisfy all of the allOf schemas",
            describe(path)
        ),
        ("anyOf", _) => format!("{}: matches none of the anyOf schemas", describe(path)),
        (_, _) => format!(
            "{}: matches {matches} of the oneOf schemas (exactly one required)",
            describe(path)
        ),
    })
}

/// `enum`: the value must deep-equal one of the listed values.
fn check_enum(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<(), String> {
    let Some(enum_schema) = schema.get("enum") else {
        return Ok(());
    };
    let Some(allowed) = enum_schema.as_array() else {
        return Err(format!(
            "{}: schema 'enum' must be an array",
            describe(path)
        ));
    };
    if allowed.iter().any(|v| v == value) {
        return Ok(());
    }
    let listed = allowed
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("{}: expected one of [{}]", describe(path), listed))
}

/// `const`: the value must deep-equal the constant.
fn check_const(schema: &Map<String, Value>, value: &Value, path: &str) -> Result<(), String> {
    match schema.get("const") {
        None => Ok(()),
        Some(expected) if expected == value => Ok(()),
        Some(expected) => Err(format!(
            "{}: expected the constant value {expected}",
            describe(path)
        )),
    }
}

/// Human name of the value being checked: the arguments object at the root, else a
/// field path like `field 'address.city'`.
fn describe(path: &str) -> String {
    if path.is_empty() {
        "arguments".to_string()
    } else {
        format!("field '{path}'")
    }
}

/// Prefix for errors about a path's contents: `field 'address': `.
fn path_prefix(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{}: ", describe(path))
    }
}

/// Path of a named child field.
fn field(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// Path of an indexed array element.
fn element(path: &str, index: usize) -> String {
    let base = if path.is_empty() { "arguments" } else { path };
    format!("{base}[{index}]")
}

/// serde_json name of a value's JSON type, for messages.
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Whether `value` has the JSON type `name`. Unknown names are rejected before this
/// runs (see [`KNOWN_TYPES`]).
fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        // A whole-valued float is an integer (JSON Schema semantics); `1.5` is not.
        "integer" => match value {
            Value::Number(n) => {
                n.is_i64()
                    || n.is_u64()
                    || n.as_f64()
                        .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ok(schema: Value, args: Value) -> bool {
        validate_arguments(Some(&schema), &args).is_ok()
    }

    fn err(schema: Value, args: Value) -> String {
        validate_arguments(Some(&schema), &args).unwrap_err()
    }

    #[test]
    fn no_schema_or_empty_schema_accepts_anything() {
        let args = json!({"any": ["thing", 1, null, {"x": true}]});
        assert!(validate_arguments(None, &args).is_ok());
        assert!(ok(json!({}), args.clone()));
        assert!(ok(json!(true), args.clone()));
        assert!(ok(Value::Null, args));
    }

    #[test]
    fn a_false_schema_rejects_everything() {
        assert!(validate_arguments(Some(&json!(false)), &json!({})).is_err());
    }

    #[test]
    fn leaf_types_are_enforced() {
        let schema = |t| json!({"type": t});
        assert!(ok(schema("string"), json!("hi")));
        assert!(!ok(schema("string"), json!(5)));
        assert!(ok(schema("boolean"), json!(true)));
        assert!(!ok(schema("boolean"), json!("true")));
        assert!(ok(schema("null"), json!(null)));
        assert!(ok(schema("number"), json!(1.5)));
        assert!(ok(schema("array"), json!([1])));
        assert!(ok(schema("object"), json!({})));
    }

    #[test]
    fn integer_accepts_whole_numbers_only() {
        let schema = json!({"type": "integer"});
        assert!(ok(schema.clone(), json!(30)));
        assert!(ok(schema.clone(), json!(30.0)));
        assert!(!ok(schema, json!(30.5)));
    }

    #[test]
    fn type_arrays_match_any_listed_type() {
        let schema = json!({"type": ["string", "null"]});
        assert!(ok(schema.clone(), json!("hi")));
        assert!(ok(schema.clone(), json!(null)));
        assert!(!ok(schema, json!(5)));
    }

    #[test]
    fn an_unknown_type_keyword_fails_closed() {
        assert!(err(json!({"type": "wobble"}), json!("hi")).contains("unsupported type"));
    }

    #[test]
    fn required_fields_must_be_present() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string" },
            },
            "required": ["title", "start"],
        });
        assert!(ok(
            schema.clone(),
            json!({"title": "t", "start": "s", "x": 1})
        ));
        let e = err(schema, json!({"title": "t"}));
        assert!(e.contains("missing required field 'start'"), "{e}");
    }

    #[test]
    fn nested_objects_are_validated_recursively_with_paths() {
        let schema = json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"],
                },
            },
            "required": ["address"],
        });
        assert!(ok(schema.clone(), json!({"address": {"city": "Oslo"}})));
        let e = err(schema, json!({"address": {}}));
        assert!(e.contains("field 'address'") && e.contains("'city'"), "{e}");
    }

    #[test]
    fn arrays_validate_each_item() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } },
            },
        });
        assert!(ok(schema.clone(), json!({"tags": ["a", "b"]})));
        let e = err(schema, json!({"tags": ["a", 2]}));
        assert!(e.contains("field 'tags[1]'"), "{e}");
    }

    #[test]
    fn arrays_of_objects_validate_nested_items() {
        let schema = json!({
            "type": "object",
            "properties": {
                "people": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "age": { "type": "integer" } },
                        "required": ["age"],
                    },
                },
            },
        });
        assert!(ok(
            schema.clone(),
            json!({"people": [{"age": 1}, {"age": 2}]})
        ));
        let e = err(schema, json!({"people": [{"age": 1}, {}]}));
        assert!(e.contains("field 'people[1]'"), "{e}");
    }

    #[test]
    fn additional_properties_rules_are_enforced() {
        let base = json!({"type": "object", "properties": {"a": {"type": "string"}}});
        // Absent or true: extra fields allowed.
        assert!(ok(base.clone(), json!({"a": "x", "extra": 1})));
        assert!(ok(
            json!({"type": "object", "properties": {"a": {}}, "additionalProperties": true}),
            json!({"a": "x", "extra": 1})
        ));
        // False: extra fields rejected.
        let strict = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "additionalProperties": false,
        });
        assert!(ok(strict.clone(), json!({"a": "x"})));
        let e = err(strict, json!({"a": "x", "b": 1}));
        assert!(e.contains("unexpected field 'b'"), "{e}");
        // A schema: extra fields validated against it.
        let typed = json!({
            "type": "object",
            "properties": {"a": {}},
            "additionalProperties": { "type": "integer" },
        });
        assert!(ok(typed.clone(), json!({"a": "x", "n": 3})));
        assert!(!ok(typed, json!({"a": "x", "n": "3"})));
    }

    #[test]
    fn enums_are_enforced_with_the_allowed_values_listed() {
        let schema = json!({
            "type": "object",
            "properties": {
                "visibility": { "type": "string", "enum": ["public", "private"] },
            },
        });
        assert!(ok(schema.clone(), json!({"visibility": "public"})));
        let e = err(schema, json!({"visibility": "secret"}));
        assert!(e.contains("\"public\"") && e.contains("\"private\""), "{e}");
    }

    #[test]
    fn enum_objects_and_root_enums_work() {
        let root = json!({"enum": [{"a": 1}, [1, 2], "x", null]});
        assert!(ok(root.clone(), json!({"a": 1})));
        assert!(ok(root.clone(), json!([1, 2])));
        assert!(ok(root.clone(), json!("x")));
        assert!(ok(root.clone(), json!(null)));
        assert!(!ok(root, json!({"a": 2})));
    }

    #[test]
    fn const_is_enforced() {
        let schema = json!({"type": "object", "properties": {"v": {"const": 7}}});
        assert!(ok(schema.clone(), json!({"v": 7})));
        assert!(!ok(schema, json!({"v": 8})));
    }

    #[test]
    fn combinators_are_enforced() {
        let str_or_num = json!({"anyOf": [{"type": "string"}, {"type": "number"}]});
        assert!(ok(str_or_num.clone(), json!("x")));
        assert!(ok(str_or_num.clone(), json!(1)));
        assert!(!ok(str_or_num, json!([1])));

        let exactly_one =
            json!({"oneOf": [{"type": "string"}, {"type": "string", "minLength": 0}]});
        // Both branches match a string → oneOf fails; minLength is ignored metadata.
        assert!(!ok(exactly_one, json!("x")));

        let all = json!({"allOf": [{"type": "string"}, {"enum": ["a", "b"]}]});
        assert!(ok(all.clone(), json!("a")));
        assert!(!ok(all, json!("c")));
    }

    #[test]
    fn unsupported_structural_keywords_fail_closed() {
        for keyword in ["$ref", "not", "if", "patternProperties", "prefixItems"] {
            let schema = json!({ keyword: {"type": "string"} });
            let e = err(schema, json!({}));
            assert!(e.contains("unsupported keyword"), "{keyword}: {e}");
        }
    }

    #[test]
    fn tuple_style_items_fail_closed() {
        let schema = json!({"type": "array", "items": [{"type": "string"}]});
        let e = err(schema, json!(["a"]));
        assert!(e.contains("tuple-style"), "{e}");
    }

    #[test]
    fn annotations_are_ignored_but_unsupported_assertions_fail_closed() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {
                    "type": "string",
                    "description": "descriptions cost tokens",
                    "title": "A",
                    "default": "fallback",
                },
            },
        });
        assert!(ok(schema, json!({"a": "short"})));

        for keyword in ["format", "minLength", "minimum", "pattern"] {
            let schema = json!({"type": "string", keyword: "unsupported"});
            let e = err(schema, json!("anything"));
            assert!(e.contains("unsupported keyword"), "{keyword}: {e}");
        }
    }

    #[test]
    fn malformed_schema_shapes_fail_closed() {
        for (schema, message) in [
            (
                json!({"type": "string", "enum": "x"}),
                "'enum' must be an array",
            ),
            (
                json!({"type": "object", "required": "x"}),
                "'required' must be an array",
            ),
            (
                json!({"type": "object", "required": [1]}),
                "entries must be strings",
            ),
            (json!({"anyOf": {}}), "'anyOf' must be an array"),
            (
                json!({"type": "object", "additionalProperties": 1}),
                "must be an object or boolean",
            ),
            (
                json!({"type": "array", "items": null}),
                "must be an object or boolean",
            ),
        ] {
            let e = err(schema, json!({}));
            assert!(e.contains(message), "{message}: {e}");
        }
    }

    #[test]
    fn a_pathologically_nested_schema_fails_instead_of_overflowing() {
        let mut schema = json!({"type": "object"});
        for _ in 0..400 {
            schema = json!({"type": "object", "properties": {"n": schema}});
        }
        let args = json!({"n": {}});
        assert!(validate_arguments(Some(&schema), &args).is_err());
    }
}
