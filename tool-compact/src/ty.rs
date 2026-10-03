//! The internal type model shared by the encoder, the definitions parser and the validator.
//!
//! Everything goes through [`Ty`]: a JSON Schema is accepted only if it maps onto it exactly, so
//! the compact text, the validator and `decode_tools` cannot disagree about what a tool accepts.

use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    /// `{}`: any JSON value.
    Any,
    Str(Option<Format>),
    Int,
    Num,
    Bool,
    Null,
    /// Allowed literal values (strings or numbers only, so literals never collide with keywords).
    Enum(Vec<Value>),
    Arr(Box<Ty>),
    Obj(Obj),
    /// `type: [T, "null"]`.
    Nullable(Box<Ty>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    DateTime,
    Date,
    Email,
    Uri,
}

impl Format {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "date-time" => Format::DateTime,
            "date" => Format::Date,
            "email" => Format::Email,
            "uri" => Format::Uri,
            _ => return None,
        })
    }

    pub(crate) fn schema_name(self) -> &'static str {
        match self {
            Format::DateTime => "date-time",
            Format::Date => "date",
            Format::Email => "email",
            Format::Uri => "uri",
        }
    }

    /// The compact type keyword.
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Format::DateTime => "datetime",
            Format::Date => "date",
            Format::Email => "email",
            Format::Uri => "uri",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Obj {
    pub fields: Vec<Field>,
    /// Whether keys beyond `fields` are accepted.
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub name: String,
    pub ty: Ty,
    pub required: bool,
    pub desc: Option<String>,
}

/// Keywords that carry no validation meaning and are dropped.
const ANNOTATIONS: &[&str] = &["description", "title", "$schema"];
const STRUCTURAL: &[&str] = &[
    "type",
    "properties",
    "required",
    "items",
    "enum",
    "format",
    "additionalProperties",
];

/// Tool and field names the grammar can carry unambiguously.
pub(crate) fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Collapse all whitespace so a description always fits on one line.
pub(crate) fn clean_description(s: &str) -> Option<String> {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// A tool's `parameters` schema as an [`Obj`]. `None` means the tool takes no arguments.
pub(crate) fn params_from_schema(schema: Option<&Value>) -> Result<Obj, String> {
    match schema {
        None => Ok(Obj {
            fields: Vec::new(),
            open: false,
        }),
        Some(v) => match from_schema(v, "parameters")? {
            Ty::Obj(o) => Ok(o),
            _ => Err("parameters must be an object schema".into()),
        },
    }
}

pub(crate) fn from_schema(v: &Value, path: &str) -> Result<Ty, String> {
    let Some(map) = v.as_object() else {
        return Err(format!("{path}: schema must be an object"));
    };
    if let Some(k) = map
        .keys()
        .find(|k| !ANNOTATIONS.contains(&k.as_str()) && !STRUCTURAL.contains(&k.as_str()))
    {
        return Err(format!("{path}: keyword `{k}` is not supported"));
    }

    if let Some(e) = map.get("enum") {
        return enum_from_schema(map, e, path);
    }

    match map.get("type") {
        None if map.contains_key("properties") => object_from_schema(map, path),
        None => {
            forbid(
                map,
                &["items", "format", "required", "additionalProperties"],
                path,
            )?;
            Ok(Ty::Any)
        }
        Some(Value::String(t)) => base_from_schema(t, map, path),
        Some(Value::Array(ts)) => {
            let names: Option<Vec<&str>> = ts.iter().map(Value::as_str).collect();
            match names.as_deref() {
                Some([t, "null"] | ["null", t]) if *t != "null" => {
                    Ok(Ty::Nullable(Box::new(base_from_schema(t, map, path)?)))
                }
                _ => Err(format!(
                    "{path}: only `[T, \"null\"]` type unions are supported"
                )),
            }
        }
        Some(_) => Err(format!("{path}: `type` must be a string or array")),
    }
}

fn forbid(map: &Map<String, Value>, keys: &[&str], path: &str) -> Result<(), String> {
    match keys.iter().find(|k| map.contains_key(**k)) {
        Some(k) => Err(format!("{path}: `{k}` is not valid here")),
        None => Ok(()),
    }
}

fn enum_from_schema(map: &Map<String, Value>, e: &Value, path: &str) -> Result<Ty, String> {
    forbid(
        map,
        &[
            "properties",
            "items",
            "format",
            "required",
            "additionalProperties",
        ],
        path,
    )?;
    let values = e
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| format!("{path}: `enum` must be a non-empty array"))?;
    if !values.iter().all(|v| v.is_string() || v.is_number()) {
        return Err(format!("{path}: enum values must be strings or numbers"));
    }
    if let Some(t) = map.get("type") {
        let ok = match t.as_str() {
            Some("string") => values.iter().all(Value::is_string),
            Some("integer") => values.iter().all(|v| v.is_i64() || v.is_u64()),
            Some("number") => values.iter().all(Value::is_number),
            _ => false,
        };
        if !ok {
            return Err(format!("{path}: enum values do not match `type`"));
        }
    }
    Ok(Ty::Enum(values.clone()))
}

fn base_from_schema(t: &str, map: &Map<String, Value>, path: &str) -> Result<Ty, String> {
    match t {
        "string" => {
            forbid(
                map,
                &["properties", "items", "required", "additionalProperties"],
                path,
            )?;
            match map.get("format") {
                None => Ok(Ty::Str(None)),
                Some(f) => f
                    .as_str()
                    .and_then(Format::parse)
                    .map(|f| Ty::Str(Some(f)))
                    .ok_or_else(|| format!("{path}: format {f} is not supported")),
            }
        }
        "integer" | "number" | "boolean" | "null" => {
            forbid(
                map,
                &[
                    "properties",
                    "items",
                    "required",
                    "additionalProperties",
                    "format",
                ],
                path,
            )?;
            Ok(match t {
                "integer" => Ty::Int,
                "number" => Ty::Num,
                "boolean" => Ty::Bool,
                _ => Ty::Null,
            })
        }
        "array" => {
            forbid(
                map,
                &["properties", "required", "additionalProperties", "format"],
                path,
            )?;
            match map.get("items") {
                None => Ok(Ty::Arr(Box::new(Ty::Any))),
                Some(items @ Value::Object(_)) => {
                    Ok(Ty::Arr(Box::new(from_schema(items, &format!("{path}[]"))?)))
                }
                Some(_) => Err(format!("{path}: tuple `items` are not supported")),
            }
        }
        "object" => object_from_schema(map, path),
        other => Err(format!("{path}: unknown type `{other}`")),
    }
}

fn object_from_schema(map: &Map<String, Value>, path: &str) -> Result<Ty, String> {
    forbid(map, &["items", "format"], path)?;
    let props = match map.get("properties") {
        None => None,
        Some(Value::Object(p)) => Some(p),
        Some(_) => return Err(format!("{path}: `properties` must be an object")),
    };
    let required: Vec<&str> = match map.get("required") {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(Value::as_str)
            .collect::<Option<_>>()
            .ok_or_else(|| format!("{path}: `required` must list strings"))?,
        Some(_) => return Err(format!("{path}: `required` must be an array")),
    };
    let open = match map.get("additionalProperties") {
        None => props.is_none(),
        Some(Value::Bool(b)) => *b,
        Some(_) => {
            return Err(format!(
                "{path}: schema-valued `additionalProperties` is not supported"
            ));
        }
    };

    let mut fields = Vec::new();
    for (name, sub) in props.into_iter().flatten() {
        if !is_valid_name(name) {
            return Err(format!("{path}: property name `{name}` is not supported"));
        }
        fields.push(Field {
            name: name.clone(),
            ty: from_schema(sub, &format!("{path}.{name}"))?,
            required: required.contains(&name.as_str()),
            desc: sub
                .get("description")
                .and_then(Value::as_str)
                .and_then(clean_description),
        });
    }
    if let Some(r) = required
        .iter()
        .find(|r| !fields.iter().any(|f| f.name == **r))
    {
        return Err(format!(
            "{path}: required field `{r}` is not in `properties`"
        ));
    }
    // Canonical order: required fields first, like a function signature; then by name.
    fields.sort_by_key(|f| !f.required);
    Ok(Ty::Obj(Obj { fields, open }))
}

/// Rebuild a normalized JSON Schema. Annotations other than field descriptions are not kept.
pub(crate) fn to_schema(ty: &Ty) -> Value {
    match ty {
        Ty::Any => json!({}),
        Ty::Str(None) => json!({"type": "string"}),
        Ty::Str(Some(f)) => json!({"type": "string", "format": f.schema_name()}),
        Ty::Int => json!({"type": "integer"}),
        Ty::Num => json!({"type": "number"}),
        Ty::Bool => json!({"type": "boolean"}),
        Ty::Null => json!({"type": "null"}),
        Ty::Enum(values) => {
            let mut m = Map::new();
            let t = if values.iter().all(Value::is_string) {
                Some("string")
            } else if values.iter().all(|v| v.is_i64() || v.is_u64()) {
                Some("integer")
            } else if values.iter().all(Value::is_number) {
                Some("number")
            } else {
                None
            };
            if let Some(t) = t {
                m.insert("type".into(), t.into());
            }
            m.insert("enum".into(), Value::Array(values.clone()));
            Value::Object(m)
        }
        Ty::Arr(inner) => match inner.as_ref() {
            Ty::Any => json!({"type": "array"}),
            inner => json!({"type": "array", "items": to_schema(inner)}),
        },
        Ty::Obj(o) => obj_to_schema(o),
        Ty::Nullable(inner) => {
            let mut v = to_schema(inner);
            if let Some(m) = v.as_object_mut()
                && let Some(t) = m.get("type").cloned()
            {
                m.insert("type".into(), json!([t, "null"]));
            }
            v
        }
    }
}

pub(crate) fn obj_to_schema(o: &Obj) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), "object".into());
    if o.fields.is_empty() && o.open {
        return Value::Object(m);
    }
    let mut props = Map::new();
    for f in &o.fields {
        let mut s = to_schema(&f.ty);
        if let (Some(d), Some(sm)) = (&f.desc, s.as_object_mut()) {
            sm.insert("description".into(), d.clone().into());
        }
        props.insert(f.name.clone(), s);
    }
    m.insert("properties".into(), Value::Object(props));
    let required: Vec<Value> = o
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.name.clone().into())
        .collect();
    if !required.is_empty() {
        m.insert("required".into(), Value::Array(required));
    }
    if o.open {
        m.insert("additionalProperties".into(), true.into());
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calendar() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })
    }

    #[test]
    fn maps_calendar_schema() {
        let Ty::Obj(o) = from_schema(&calendar(), "p").unwrap() else {
            panic!()
        };
        assert!(!o.open);
        let f = |n: &str| o.fields.iter().find(|f| f.name == n).unwrap();
        assert_eq!(f("start").ty, Ty::Str(Some(Format::DateTime)));
        assert!(f("title").required && !f("duration_min").required);
        assert_eq!(f("attendees").ty, Ty::Arr(Box::new(Ty::Str(None))));
        assert_eq!(
            f("visibility").ty,
            Ty::Enum(vec![json!("public"), json!("private")])
        );
    }

    #[test]
    fn schema_round_trips_through_to_schema() {
        let ty = from_schema(&calendar(), "p").unwrap();
        assert_eq!(from_schema(&to_schema(&ty), "p").unwrap(), ty);
    }

    #[test]
    fn rejects_unsupported_features() {
        for bad in [
            json!({"type": "string", "pattern": "^a"}),
            json!({"type": "integer", "minimum": 1}),
            json!({"oneOf": [{"type": "string"}]}),
            json!({"$ref": "#/defs/x"}),
            json!({"type": "string", "format": "ipv4"}),
            json!({"type": ["string", "integer"]}),
            json!({"type": "array", "items": [{"type": "string"}]}),
            json!({"type": "object", "additionalProperties": {"type": "string"}}),
            json!({"type": "object", "properties": {"a b": {"type": "string"}}}),
            json!({"type": "object", "properties": {}, "required": ["x"]}),
            json!({"enum": [true, false]}),
            json!({"type": "string", "default": "x"}),
        ] {
            assert!(from_schema(&bad, "p").is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn nullable_and_open_objects() {
        assert_eq!(
            from_schema(&json!({"type": ["integer", "null"]}), "p").unwrap(),
            Ty::Nullable(Box::new(Ty::Int))
        );
        assert_eq!(
            from_schema(&json!({"type": "object"}), "p").unwrap(),
            Ty::Obj(Obj {
                fields: vec![],
                open: true
            })
        );
    }

    #[test]
    fn names_and_descriptions() {
        assert!(is_valid_name("create_calendar_event") && is_valid_name("a.b-c"));
        assert!(!is_valid_name("") && !is_valid_name("a b") && !is_valid_name("-x"));
        assert_eq!(
            clean_description("  two\n\nlines  ").as_deref(),
            Some("two lines")
        );
        assert_eq!(clean_description(" \n "), None);
    }
}
