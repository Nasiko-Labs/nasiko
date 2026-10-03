//! The supported JSON-Schema subset as a typed tree, both directions, plus argument validation.
//!
//! [`Node`] is the single representation the signature grammar renders from, parses into, and
//! validates against — so what the model is shown and what its calls are checked against cannot
//! drift apart.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Number, Value};

use crate::formats;

/// Maximum nesting of arrays/objects in a schema or a signature. Deeper input is bypassed
/// (schemas) or rejected (signatures) instead of recursing without bound.
///
/// Depth rule, shared by both sides: a top-level field's node is depth 1, and array items and
/// object fields are one deeper than their parent. A node deeper than `MAX_DEPTH` is refused;
/// an empty object at exactly `MAX_DEPTH` is allowed.
pub const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    /// A string, optionally with a non-date `format` kept as an annotation (`str<email>`).
    Str(Option<String>),
    Int,
    Num,
    Bool,
    DateTime,
    Date,
    Enum(Vec<String>),
    Array(Box<Node>),
    Object(Obj),
}

/// An object's fields and whether `additionalProperties: false` was set (`closed`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Obj {
    pub(crate) fields: Vec<Field>,
    pub(crate) closed: bool,
}

/// Value constraints kept as compact annotations. Which ones may appear depends on the type;
/// [`node_from_schema`] enforces that.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Cons {
    pub(crate) minimum: Option<Number>,
    pub(crate) maximum: Option<Number>,
    pub(crate) exclusive_minimum: Option<Number>,
    pub(crate) exclusive_maximum: Option<Number>,
    pub(crate) min_length: Option<u64>,
    pub(crate) max_length: Option<u64>,
    pub(crate) min_items: Option<u64>,
    pub(crate) max_items: Option<u64>,
    pub(crate) unique_items: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub(crate) ty: Ty,
    pub(crate) cons: Cons,
    pub(crate) desc: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) required: bool,
    pub(crate) node: Node,
}

/// Why a schema is outside the subset. Not an error: it routes the tool list to bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unsupported {
    pub(crate) path: String,
    pub(crate) detail: String,
}

fn unsupported<T>(path: &str, detail: impl Into<String>) -> Result<T, Unsupported> {
    Err(Unsupported {
        path: path.to_string(),
        detail: detail.into(),
    })
}

/// Tool, argument and format names: `[A-Za-z0-9_-]+`.
pub(crate) fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

pub(crate) fn is_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_name_char)
}

/// Enum values are rendered bare, so they are restricted to chars that cannot be confused with
/// the grammar's delimiters (`|`, `,`, `)`, `]`, `}`, `<`, `>`, `"`, whitespace).
pub(crate) fn is_enum_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | '+' | '@')
}

/// `pattern` is deliberately absent: without a regex engine it could be shown but not enforced,
/// and an unenforced limit is not fail-closed. It bypasses as an unknown keyword.
const STRING_CONS: [&str; 2] = ["minLength", "maxLength"];
const NUMBER_CONS: [&str; 4] = ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"];
const ARRAY_CONS: [&str; 3] = ["minItems", "maxItems", "uniqueItems"];
const BASE_KEYWORDS: [&str; 8] = [
    "type",
    "description",
    "enum",
    "format",
    "items",
    "properties",
    "required",
    "additionalProperties",
];

fn is_known_keyword(key: &str) -> bool {
    BASE_KEYWORDS.contains(&key)
        || STRING_CONS.contains(&key)
        || NUMBER_CONS.contains(&key)
        || ARRAY_CONS.contains(&key)
}

/// Top-level `parameters`: an object schema with `properties`, optional `required` and optional
/// `additionalProperties: false`. `None` means "no arguments".
pub(crate) fn params_from_schema(params: Option<&Value>) -> Result<Obj, Unsupported> {
    let Some(params) = params else {
        return Ok(Obj {
            fields: Vec::new(),
            closed: false,
        });
    };
    let path = "$";
    let Some(obj) = params.as_object() else {
        return unsupported(path, "parameters is not an object");
    };
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "type" | "properties" | "required" | "additionalProperties"
        ) {
            return unsupported(path, format!("keyword '{key}'"));
        }
    }
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return unsupported(path, "parameters type must be \"object\"");
    }
    object_from_schema(obj, path, 1)
}

fn object_from_schema(
    obj: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<Obj, Unsupported> {
    // A bare `{"type":"object"}` means "any object" — not the same as "no properties".
    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        return unsupported(path, "object without a 'properties' map");
    };
    let closed = match obj.get("additionalProperties") {
        None => false,
        Some(Value::Bool(false)) => true,
        Some(other) => return unsupported(path, format!("additionalProperties {other}")),
    };
    let props: BTreeMap<&String, &Value> = props.iter().collect();
    let required: Vec<&str> = match obj.get("required") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut names = Vec::new();
            let mut seen = BTreeSet::new();
            for item in items {
                let Some(name) = item.as_str() else {
                    return unsupported(path, "non-string entry in 'required'");
                };
                if !seen.insert(name) {
                    return unsupported(path, format!("duplicate required '{name}'"));
                }
                if !props.keys().any(|k| k.as_str() == name) {
                    return unsupported(path, format!("required '{name}' is not a property"));
                }
                names.push(name);
            }
            names
        }
        Some(_) => return unsupported(path, "'required' is not an array"),
    };

    let mut fields = Vec::with_capacity(props.len());
    // Required first, in the author's `required` order (round-trips exactly), then optional by name.
    for name in &required {
        if let Some((key, schema)) = props.iter().find(|(k, _)| k.as_str() == *name) {
            fields.push(field(key, true, schema, path, depth)?);
        }
    }
    for (key, schema) in &props {
        if !required.contains(&key.as_str()) {
            fields.push(field(key, false, schema, path, depth)?);
        }
    }
    Ok(Obj { fields, closed })
}

fn field(
    name: &str,
    required: bool,
    schema: &Value,
    parent: &str,
    depth: usize,
) -> Result<Field, Unsupported> {
    let path = format!("{parent}.{name}");
    if !is_name(name) {
        return unsupported(&path, "property name outside [A-Za-z0-9_-]");
    }
    Ok(Field {
        name: name.to_string(),
        required,
        node: node_from_schema(schema, &path, depth)?,
    })
}

pub(crate) fn node_from_schema(v: &Value, path: &str, depth: usize) -> Result<Node, Unsupported> {
    if depth > MAX_DEPTH {
        return unsupported(path, format!("nested deeper than {MAX_DEPTH}"));
    }
    let Some(obj) = v.as_object() else {
        return unsupported(path, "schema is not an object");
    };
    for key in obj.keys() {
        if !is_known_keyword(key) {
            return unsupported(path, format!("keyword '{key}'"));
        }
    }
    let desc = match obj.get("description") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return unsupported(path, "non-string description"),
    };
    let ty_name = match obj.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(_) => return unsupported(path, "'type' is not a single string"),
        None => return unsupported(path, "missing 'type'"),
    };

    let mut allowed: Vec<&str> = vec!["type", "description"];
    let ty = match ty_name {
        "string" if obj.contains_key("enum") => {
            allowed.push("enum");
            enum_values(obj.get("enum"), path)?
        }
        "string" => {
            allowed.push("format");
            allowed.extend(STRING_CONS);
            match obj.get("format") {
                None => Ty::Str(None),
                Some(Value::String(f)) if f == "date-time" => Ty::DateTime,
                Some(Value::String(f)) if f == "date" => Ty::Date,
                Some(Value::String(f)) if is_name(f) => Ty::Str(Some(f.clone())),
                Some(other) => return unsupported(path, format!("format {other}")),
            }
        }
        "integer" | "number" => {
            allowed.extend(NUMBER_CONS);
            if ty_name == "integer" {
                Ty::Int
            } else {
                Ty::Num
            }
        }
        "boolean" => Ty::Bool,
        "array" => {
            allowed.push("items");
            allowed.extend(ARRAY_CONS);
            let Some(items) = obj.get("items") else {
                return unsupported(path, "array without 'items'");
            };
            Ty::Array(Box::new(node_from_schema(
                items,
                &format!("{path}[]"),
                depth + 1,
            )?))
        }
        "object" => {
            allowed.extend(["properties", "required", "additionalProperties"]);
            Ty::Object(object_from_schema(obj, path, depth + 1)?)
        }
        other => return unsupported(path, format!("type '{other}'")),
    };
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return unsupported(path, format!("keyword '{key}' on type '{ty_name}'"));
        }
    }
    Ok(Node {
        ty,
        cons: cons_from_schema(obj, path)?,
        desc,
    })
}

fn cons_from_schema(obj: &Map<String, Value>, path: &str) -> Result<Cons, Unsupported> {
    let number = |key: &str| -> Result<Option<Number>, Unsupported> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) => Ok(Some(n.clone())),
            Some(other) => unsupported(path, format!("{key} {other}")),
        }
    };
    let count = |key: &str| -> Result<Option<u64>, Unsupported> {
        match obj.get(key) {
            None => Ok(None),
            Some(Value::Number(n)) if n.is_u64() => Ok(n.as_u64()),
            Some(other) => unsupported(path, format!("{key} {other}")),
        }
    };
    Ok(Cons {
        minimum: number("minimum")?,
        maximum: number("maximum")?,
        exclusive_minimum: number("exclusiveMinimum")?,
        exclusive_maximum: number("exclusiveMaximum")?,
        min_length: count("minLength")?,
        max_length: count("maxLength")?,
        min_items: count("minItems")?,
        max_items: count("maxItems")?,
        unique_items: match obj.get("uniqueItems") {
            None => false,
            Some(Value::Bool(true)) => true,
            // `false` is the default; keeping it would need a second spelling for one meaning.
            Some(other) => return unsupported(path, format!("uniqueItems {other}")),
        },
    })
}

fn enum_values(v: Option<&Value>, path: &str) -> Result<Ty, Unsupported> {
    let Some(Value::Array(items)) = v else {
        return unsupported(path, "'enum' is not an array");
    };
    if items.is_empty() {
        return unsupported(path, "empty enum");
    }
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str() else {
            return unsupported(path, "non-string enum value");
        };
        if s.is_empty() || !s.chars().all(is_enum_char) {
            return unsupported(path, format!("enum value {item} needs quoting"));
        }
        if values.iter().any(|v| v == s) {
            return unsupported(path, format!("duplicate enum value {item}"));
        }
        values.push(s.to_string());
    }
    Ok(Ty::Enum(values))
}

/// Inverse of [`params_from_schema`].
pub(crate) fn params_to_schema(obj: &Obj) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), Value::String("object".into()));
    object_into(obj, &mut out);
    Value::Object(out)
}

fn object_into(obj: &Obj, out: &mut Map<String, Value>) {
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in &obj.fields {
        props.insert(f.name.clone(), node_to_schema(&f.node));
        if f.required {
            required.push(Value::String(f.name.clone()));
        }
    }
    out.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    if obj.closed {
        out.insert("additionalProperties".into(), Value::Bool(false));
    }
}

/// Inverse of [`node_from_schema`].
pub(crate) fn node_to_schema(node: &Node) -> Value {
    let mut obj = Map::new();
    let s = |v: &str| Value::String(v.to_string());
    match &node.ty {
        Ty::Str(format) => {
            obj.insert("type".into(), s("string"));
            if let Some(f) = format {
                obj.insert("format".into(), s(f));
            }
        }
        Ty::DateTime => {
            obj.insert("type".into(), s("string"));
            obj.insert("format".into(), s("date-time"));
        }
        Ty::Date => {
            obj.insert("type".into(), s("string"));
            obj.insert("format".into(), s("date"));
        }
        Ty::Enum(values) => {
            obj.insert("type".into(), s("string"));
            obj.insert(
                "enum".into(),
                Value::Array(values.iter().map(|v| s(v)).collect()),
            );
        }
        Ty::Int => {
            obj.insert("type".into(), s("integer"));
        }
        Ty::Num => {
            obj.insert("type".into(), s("number"));
        }
        Ty::Bool => {
            obj.insert("type".into(), s("boolean"));
        }
        Ty::Array(inner) => {
            obj.insert("type".into(), s("array"));
            obj.insert("items".into(), node_to_schema(inner));
        }
        Ty::Object(o) => {
            obj.insert("type".into(), s("object"));
            object_into(o, &mut obj);
        }
    }
    let c = &node.cons;
    let mut put = |key: &str, v: Option<Value>| {
        if let Some(v) = v {
            obj.insert(key.into(), v);
        }
    };
    put("minimum", c.minimum.clone().map(Value::Number));
    put("maximum", c.maximum.clone().map(Value::Number));
    put(
        "exclusiveMinimum",
        c.exclusive_minimum.clone().map(Value::Number),
    );
    put(
        "exclusiveMaximum",
        c.exclusive_maximum.clone().map(Value::Number),
    );
    put("minLength", c.min_length.map(Value::from));
    put("maxLength", c.max_length.map(Value::from));
    put("minItems", c.min_items.map(Value::from));
    put("maxItems", c.max_items.map(Value::from));
    put("uniqueItems", c.unique_items.then_some(Value::Bool(true)));
    if let Some(desc) = &node.desc {
        obj.insert("description".into(), Value::String(desc.clone()));
    }
    Value::Object(obj)
}

/// Validate a call's arguments object against the tool's parameters. Returns a human-readable
/// reason on the first violation; never coerces.
pub(crate) fn validate_args(obj: &Obj, args: &Value) -> Result<(), String> {
    validate_object(obj, args, "$")
}

/// Undeclared keys are rejected whether or not the schema is `closed`: a guessed argument is a
/// failure, not something to forward.
fn validate_object(obj: &Obj, value: &Value, path: &str) -> Result<(), String> {
    let Some(map) = value.as_object() else {
        return Err(format!("{path}: expected object"));
    };
    for f in &obj.fields {
        if f.required && !map.contains_key(&f.name) {
            return Err(format!("{path}: missing required field '{}'", f.name));
        }
    }
    for (key, v) in map {
        let Some(f) = obj.fields.iter().find(|f| &f.name == key) else {
            return Err(format!("{path}: unexpected field '{key}'"));
        };
        validate(&f.node, v, &format!("{path}.{key}"))?;
    }
    Ok(())
}

fn validate(node: &Node, value: &Value, path: &str) -> Result<(), String> {
    let ok = match (&node.ty, value) {
        (Ty::Str(_), Value::String(_)) => true,
        (Ty::DateTime, Value::String(s)) => {
            if !formats::is_date_time(s) {
                return Err(format!("{path}: '{s}' is not an RFC 3339 date-time"));
            }
            true
        }
        (Ty::Date, Value::String(s)) => {
            if !formats::is_date(s) {
                return Err(format!("{path}: '{s}' is not an RFC 3339 date"));
            }
            true
        }
        (Ty::Enum(values), Value::String(s)) => {
            if !values.contains(s) {
                return Err(format!("{path}: '{s}' is not one of {}", values.join("|")));
            }
            true
        }
        // Integer literals only: `30.0` is rejected rather than silently turned into `30`.
        (Ty::Int, Value::Number(n)) => n.is_i64() || n.is_u64(),
        (Ty::Num, Value::Number(_)) | (Ty::Bool, Value::Bool(_)) => true,
        (Ty::Array(inner), Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                validate(inner, item, &format!("{path}[{i}]"))?;
            }
            true
        }
        (Ty::Object(o), Value::Object(_)) => {
            validate_object(o, value, path)?;
            true
        }
        _ => false,
    };
    if !ok {
        return Err(format!(
            "{path}: expected {}, got {value}",
            type_label(&node.ty)
        ));
    }
    validate_cons(&node.cons, value, path)
}

/// Exact when both sides are integers (`i64`/`u64`, compared as `i128`), otherwise `f64`.
/// `None` (incomparable, e.g. NaN) fails every bound.
fn compare_numbers(a: &Number, b: &Number) -> Option<Ordering> {
    let int = |n: &Number| {
        n.as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
    };
    match (int(a), int(b)) {
        (Some(x), Some(y)) => Some(x.cmp(&y)),
        _ => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}

fn validate_cons(c: &Cons, value: &Value, path: &str) -> Result<(), String> {
    if let Value::Number(x) = value {
        // (bound, name, orderings of value-vs-bound that pass)
        let checks: [(&Option<Number>, &str, &[Ordering]); 4] = [
            (&c.minimum, "minimum", &[Ordering::Greater, Ordering::Equal]),
            (&c.maximum, "maximum", &[Ordering::Less, Ordering::Equal]),
            (
                &c.exclusive_minimum,
                "exclusiveMinimum",
                &[Ordering::Greater],
            ),
            (&c.exclusive_maximum, "exclusiveMaximum", &[Ordering::Less]),
        ];
        for (bound, name, pass) in checks {
            if let Some(b) = bound
                && !compare_numbers(x, b).is_some_and(|o| pass.contains(&o))
            {
                return Err(format!("{path}: {x} violates {name} {b}"));
            }
        }
    }
    if let Value::String(s) = value {
        let len = s.chars().count() as u64;
        if c.min_length.is_some_and(|m| len < m) || c.max_length.is_some_and(|m| len > m) {
            return Err(format!("{path}: length {len} out of bounds"));
        }
    }
    if let Value::Array(items) = value {
        let len = items.len() as u64;
        if c.min_items.is_some_and(|m| len < m) || c.max_items.is_some_and(|m| len > m) {
            return Err(format!("{path}: {len} items out of bounds"));
        }
        if c.unique_items {
            for (i, a) in items.iter().enumerate() {
                if items.iter().skip(i + 1).any(|b| a == b) {
                    return Err(format!("{path}: items are not unique"));
                }
            }
        }
    }
    Ok(())
}

fn type_label(ty: &Ty) -> &'static str {
    match ty {
        Ty::Str(_) => "str",
        Ty::Int => "int",
        Ty::Num => "num",
        Ty::Bool => "bool",
        Ty::DateTime => "datetime",
        Ty::Date => "date",
        Ty::Enum(_) => "enum",
        Ty::Array(_) => "array",
        Ty::Object(_) => "object",
    }
}
