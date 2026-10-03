//! The supported JSON Schema subset, as a small typed model.
//!
//! [`Ty::from_json`] accepts exactly the subset the compact grammar can express and rejects
//! everything else with a reason, so callers bypass compaction instead of losing meaning.
//! [`Ty::to_json`] is its inverse, used by `decode_tools`.

use serde_json::{Map, Number, Value};

/// Deepest schema nesting accepted; deeper schemas are bypassed (keeps recursion bounded).
const MAX_DEPTH: usize = 32;

/// Keywords that carry no meaning for the model or the validator and are dropped.
const IGNORED_KEYWORDS: &[&str] = &["title", "$schema", "$comment", "examples"];

/// String formats the grammar names directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    DateTime,
    Date,
    Time,
    Email,
    Uri,
    Uuid,
}

impl Format {
    pub(crate) const ALL: [Format; 6] = [
        Format::DateTime,
        Format::Date,
        Format::Time,
        Format::Email,
        Format::Uri,
        Format::Uuid,
    ];

    /// The JSON Schema `format` value.
    pub(crate) fn schema_name(self) -> &'static str {
        match self {
            Format::DateTime => "date-time",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
            Format::Uuid => "uuid",
        }
    }

    /// The compact type keyword.
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Format::DateTime => "datetime",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
            Format::Uuid => "uuid",
        }
    }

    fn from_schema_name(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.schema_name() == name)
    }
}

/// Inclusive numeric bounds (`minimum` / `maximum`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Range {
    pub(crate) min: Option<Number>,
    pub(crate) max: Option<Number>,
}

impl Range {
    pub(crate) fn is_empty(&self) -> bool {
        self.min.is_none() && self.max.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    Str(Option<Format>),
    Int(Range),
    Num(Range),
    Bool,
    /// No `type`: any JSON value.
    Any,
    /// `{"type":"object"}` with no `properties`: any object.
    AnyObject,
    /// `enum` (or `const`) of scalar values.
    Enum(Vec<Value>),
    Array(Box<Ty>),
    Object(Object),
}

/// One schema node.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ty {
    pub(crate) kind: Kind,
    /// `type: [T, "null"]`.
    pub(crate) nullable: bool,
    pub(crate) description: Option<String>,
    pub(crate) default: Option<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) required: bool,
    pub(crate) ty: Ty,
}

/// An object with declared properties. Fields are ordered required-first, then by name, so the
/// rendering is deterministic whatever map order the caller's JSON used.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Object {
    pub(crate) fields: Vec<Field>,
    /// `additionalProperties: false`.
    pub(crate) closed: bool,
}

impl Object {
    pub(crate) fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub(crate) fn sort_fields(&mut self) {
        self.fields.sort_by(|a, b| {
            b.required
                .cmp(&a.required)
                .then_with(|| a.name.cmp(&b.name))
        });
    }
}

impl Ty {
    pub(crate) fn plain(kind: Kind) -> Ty {
        Ty {
            kind,
            nullable: false,
            description: None,
            default: None,
        }
    }

    /// Parses a tool's top-level `parameters` schema, which must describe an object.
    pub(crate) fn parameters_from_json(schema: &Value) -> Result<Object, String> {
        let ty = Ty::from_json(schema, 0)?;
        if ty.nullable || ty.default.is_some() {
            return Err("top-level parameters must be a plain object".into());
        }
        match ty.kind {
            Kind::Object(obj) => Ok(obj),
            Kind::AnyObject => Ok(Object {
                fields: Vec::new(),
                closed: false,
            }),
            _ => Err("top-level parameters must have type \"object\"".into()),
        }
    }

    pub(crate) fn from_json(schema: &Value, depth: usize) -> Result<Ty, String> {
        if depth > MAX_DEPTH {
            return Err(format!("nesting deeper than {MAX_DEPTH}"));
        }
        let map = schema
            .as_object()
            .ok_or_else(|| "schema node is not an object".to_string())?;
        if map.contains_key("anyOf") {
            return optional_any_of(map, depth);
        }
        let description = match map.get("description") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err("description is not a string".into()),
        };
        let default = map.get("default").cloned();
        let (type_name, nullable) = parse_type(map.get("type"))?;

        let (kind, used): (Kind, &[&str]) = if let Some(values) = enum_values(map)? {
            if let Some(t) = type_name
                && !values
                    .iter()
                    .all(|v| value_has_type(v, t) || (nullable && v.is_null()))
            {
                return Err(format!("enum values do not all match type `{t}`"));
            }
            (Kind::Enum(values), &["enum", "const", "type"])
        } else {
            match type_name {
                None => (Kind::Any, &[]),
                Some("string") => (Kind::Str(string_format(map)?), &["type", "format"]),
                Some("integer") => (Kind::Int(range(map)?), &["type", "minimum", "maximum"]),
                Some("number") => (Kind::Num(range(map)?), &["type", "minimum", "maximum"]),
                Some("boolean") => (Kind::Bool, &["type"]),
                Some("array") => (array_items(map, depth)?, &["type", "items"]),
                Some("object") => (
                    object(map, depth)?,
                    &["type", "properties", "required", "additionalProperties"],
                ),
                Some(other) => return Err(format!("type `{other}` is not supported")),
            }
        };

        for key in map.keys() {
            let known = used.contains(&key.as_str())
                || key == "description"
                || key == "default"
                || IGNORED_KEYWORDS.contains(&key.as_str());
            if !known {
                return Err(format!("keyword `{key}` is not supported here"));
            }
        }
        Ok(Ty {
            kind,
            nullable,
            description,
            default,
        })
    }

    /// Rebuilds a JSON Schema for this node.
    pub(crate) fn to_json(&self) -> Value {
        let mut out = Map::new();
        let base_type: Option<&str> = match &self.kind {
            Kind::Str(format) => {
                if let Some(f) = format {
                    out.insert("format".into(), Value::String(f.schema_name().into()));
                }
                Some("string")
            }
            Kind::Int(r) => {
                insert_range(&mut out, r);
                Some("integer")
            }
            Kind::Num(r) => {
                insert_range(&mut out, r);
                Some("number")
            }
            Kind::Bool => Some("boolean"),
            Kind::Any => None,
            Kind::AnyObject => Some("object"),
            Kind::Enum(values) => {
                out.insert("enum".into(), Value::Array(values.clone()));
                common_scalar_type(values)
            }
            Kind::Array(items) => {
                if items.kind != Kind::Any || items.description.is_some() || items.nullable {
                    out.insert("items".into(), items.to_json());
                }
                Some("array")
            }
            Kind::Object(obj) => {
                out.insert("properties".into(), object_properties(obj));
                let required: Vec<Value> = obj
                    .fields
                    .iter()
                    .filter(|f| f.required)
                    .map(|f| Value::String(f.name.clone()))
                    .collect();
                if !required.is_empty() {
                    out.insert("required".into(), Value::Array(required));
                }
                if obj.closed {
                    out.insert("additionalProperties".into(), Value::Bool(false));
                }
                Some("object")
            }
        };
        if let Some(t) = base_type {
            let ty = if self.nullable {
                Value::Array(vec![Value::String(t.into()), Value::String("null".into())])
            } else {
                Value::String(t.into())
            };
            out.insert("type".into(), ty);
        }
        if let Some(d) = &self.description {
            out.insert("description".into(), Value::String(d.clone()));
        }
        if let Some(d) = &self.default {
            out.insert("default".into(), d.clone());
        }
        Value::Object(out)
    }
}

/// `{"anyOf": [T, {"type": "null"}]}`: the shape Pydantic and FastMCP emit for `Optional[T]`.
/// Lowered to `T` made nullable; an enum gains a `null` member instead, because an enum's
/// members alone decide what is valid. `description` / `default` may sit on the wrapper.
fn optional_any_of(map: &Map<String, Value>, depth: usize) -> Result<Ty, String> {
    const WRAPPER_KEYS: &[&str] = &["anyOf", "description", "default"];
    if let Some(key) = map
        .keys()
        .find(|k| !WRAPPER_KEYS.contains(&k.as_str()) && !IGNORED_KEYWORDS.contains(&k.as_str()))
    {
        return Err(format!("keyword `{key}` next to anyOf is not supported"));
    }
    let is_null = |v: &Value| {
        v.as_object()
            .is_some_and(|o| o.len() == 1 && o.get("type").and_then(Value::as_str) == Some("null"))
    };
    let branches = map
        .get("anyOf")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let inner = match branches {
        [a, b] if is_null(b) && !is_null(a) => a,
        [a, b] if is_null(a) && !is_null(b) => b,
        _ => return Err("anyOf other than [T, {\"type\":\"null\"}] is not supported".into()),
    };
    let mut ty = Ty::from_json(inner, depth + 1)?;
    if ty.nullable {
        return Err("anyOf around an already nullable type is not supported".into());
    }
    match &mut ty.kind {
        Kind::Enum(values) => {
            if !values.contains(&Value::Null) {
                values.push(Value::Null);
            }
        }
        _ => ty.nullable = true,
    }
    match (map.get("description"), &ty.description) {
        (None, _) => {}
        (Some(Value::String(d)), None) => ty.description = Some(d.clone()),
        _ => return Err("description on both anyOf and its branch is not supported".into()),
    }
    if let Some(d) = map.get("default") {
        if ty.default.is_some() {
            return Err("default on both anyOf and its branch is not supported".into());
        }
        ty.default = Some(d.clone());
    }
    Ok(ty)
}

/// Builds the `properties` map of an object.
pub(crate) fn object_properties(obj: &Object) -> Value {
    let mut props = Map::new();
    for f in &obj.fields {
        props.insert(f.name.clone(), f.ty.to_json());
    }
    Value::Object(props)
}

fn parse_type(raw: Option<&Value>) -> Result<(Option<&str>, bool), String> {
    match raw {
        None => Ok((None, false)),
        Some(Value::String(t)) if t == "null" => Err("type `null` alone is not supported".into()),
        Some(Value::String(t)) => Ok((Some(t.as_str()), false)),
        Some(Value::Array(types)) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            match names.as_slice() {
                [t, "null"] | ["null", t] if names.len() == types.len() && *t != "null" => {
                    Ok((Some(*t), true))
                }
                _ => Err("type unions other than [T, \"null\"] are not supported".into()),
            }
        }
        Some(_) => Err("type is neither a string nor an array".into()),
    }
}

fn enum_values(map: &Map<String, Value>) -> Result<Option<Vec<Value>>, String> {
    let values = match (map.get("enum"), map.get("const")) {
        (Some(_), Some(_)) => return Err("enum and const together are not supported".into()),
        (Some(Value::Array(values)), None) => values.clone(),
        (Some(_), None) => return Err("enum is not an array".into()),
        (None, Some(v)) => vec![v.clone()],
        (None, None) => return Ok(None),
    };
    if values.is_empty() {
        return Err("enum is empty".into());
    }
    if values.iter().any(|v| v.is_array() || v.is_object()) {
        return Err("enum values must be scalars".into());
    }
    Ok(Some(values))
}

fn value_has_type(v: &Value, t: &str) -> bool {
    match t {
        "string" => v.is_string(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        _ => false,
    }
}

/// The JSON Schema type shared by every non-null enum value, if any.
fn common_scalar_type(values: &[Value]) -> Option<&'static str> {
    let mut found: Option<&'static str> = None;
    for v in values.iter().filter(|v| !v.is_null()) {
        let t = match v {
            Value::String(_) => "string",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            _ => return None,
        };
        match found {
            None => found = Some(t),
            Some(prev) if prev == t => {}
            Some(_) => return None,
        }
    }
    found
}

fn string_format(map: &Map<String, Value>) -> Result<Option<Format>, String> {
    match map.get("format") {
        None => Ok(None),
        Some(Value::String(name)) => Format::from_schema_name(name)
            .map(Some)
            .ok_or_else(|| format!("string format `{name}` is not supported")),
        Some(_) => Err("format is not a string".into()),
    }
}

fn range(map: &Map<String, Value>) -> Result<Range, String> {
    let bound = |key: &str| match map.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) => Ok(Some(n.clone())),
        Some(_) => Err(format!("{key} is not a number")),
    };
    Ok(Range {
        min: bound("minimum")?,
        max: bound("maximum")?,
    })
}

fn insert_range(out: &mut Map<String, Value>, r: &Range) {
    if let Some(n) = &r.min {
        out.insert("minimum".into(), Value::Number(n.clone()));
    }
    if let Some(n) = &r.max {
        out.insert("maximum".into(), Value::Number(n.clone()));
    }
}

fn array_items(map: &Map<String, Value>, depth: usize) -> Result<Kind, String> {
    let items = match map.get("items") {
        None => Ty::plain(Kind::Any),
        Some(v @ Value::Object(_)) => Ty::from_json(v, depth + 1)?,
        Some(_) => return Err("tuple-style items are not supported".into()),
    };
    Ok(Kind::Array(Box::new(items)))
}

fn object(map: &Map<String, Value>, depth: usize) -> Result<Kind, String> {
    let closed = match map.get("additionalProperties") {
        None | Some(Value::Bool(true)) => false,
        Some(Value::Bool(false)) => true,
        Some(_) => return Err("additionalProperties schemas are not supported".into()),
    };
    let required: Vec<&str> = match map.get("required") {
        None => Vec::new(),
        Some(Value::Array(names)) => names
            .iter()
            .map(|n| n.as_str().ok_or("required lists a non-string"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("required is not an array".into()),
    };
    let props = match map.get("properties") {
        None if required.is_empty() && !closed => return Ok(Kind::AnyObject),
        None => &Map::new(),
        Some(Value::Object(p)) => p,
        Some(_) => return Err("properties is not an object".into()),
    };
    if let Some(missing) = required.iter().find(|r| !props.contains_key(**r)) {
        return Err(format!("required field `{missing}` has no property schema"));
    }
    let mut fields = Vec::with_capacity(props.len());
    for (name, schema) in props {
        fields.push(Field {
            name: name.clone(),
            required: required.contains(&name.as_str()),
            ty: Ty::from_json(schema, depth + 1).map_err(|e| format!("{name}: {e}"))?,
        });
    }
    let mut obj = Object { fields, closed };
    obj.sort_fields();
    Ok(Kind::Object(obj))
}
