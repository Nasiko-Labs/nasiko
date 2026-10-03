//! The supported JSON Schema subset as a typed tree, and the conversions in and out of it.
//!
//! [`Node::from_json`] is the capability check: every keyword is either understood exactly or
//! rejected with [`Error::Unsupported`]. Nothing is dropped silently, so a schema that parses
//! here can always be rendered back ([`Node::to_json`]) to an equivalent JSON Schema.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::{Error, ToolDef};

/// String formats the compact form names as their own types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    DateTime,
    Date,
    Email,
    Uri,
}

impl Format {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "date-time" => Self::DateTime,
            "date" => Self::Date,
            "email" => Self::Email,
            "uri" => Self::Uri,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateTime => "date-time",
            Self::Date => "date",
            Self::Email => "email",
            Self::Uri => "uri",
        }
    }
}

/// An object's fields. `closed` is `additionalProperties: false`; absent means open.
#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub properties: BTreeMap<String, Node>,
    pub required: BTreeSet<String>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    String(Option<Format>),
    Integer,
    Number,
    Boolean,
    Array(Box<Node>),
    Object(Object),
    /// Literal choices. `typed` records whether the schema also said `"type": "string"`
    /// (or number/integer), so the round trip reproduces it.
    Enum {
        values: Vec<Value>,
        typed: Option<&'static str>,
    },
}

/// One schema node: its kind plus the description the model reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    pub description: Option<String>,
}

fn unsupported(path: &str, reason: impl Into<String>) -> Error {
    Error::Unsupported {
        path: path.to_string(),
        reason: reason.into(),
    }
}

fn child(path: &str, key: &str) -> String {
    format!("{path}.{key}")
}

impl Node {
    /// Parse a JSON Schema node, rejecting anything outside the supported subset.
    pub fn from_json(v: &Value, path: &str) -> Result<Self, Error> {
        let obj = v
            .as_object()
            .ok_or_else(|| unsupported(path, "schema is not an object"))?;
        const KNOWN: &[&str] = &[
            "type",
            "description",
            "format",
            "enum",
            "items",
            "properties",
            "required",
            "additionalProperties",
        ];
        if let Some(k) = obj.keys().find(|k| !KNOWN.contains(&k.as_str())) {
            return Err(unsupported(path, format!("keyword `{k}`")));
        }
        let description = match obj.get("description") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(unsupported(path, "non-string description")),
        };
        let ty = match obj.get("type") {
            None => None,
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => return Err(unsupported(path, "`type` is not a single string")),
        };
        let allow = |keys: &[&str]| -> Result<(), Error> {
            for k in obj.keys() {
                if !keys.contains(&k.as_str()) && k != "type" && k != "description" {
                    return Err(unsupported(path, format!("`{k}` not valid here")));
                }
            }
            Ok(())
        };

        if let Some(values) = obj.get("enum") {
            allow(&["enum"])?;
            return Ok(Node {
                kind: parse_enum(values, ty, path)?,
                description,
            });
        }

        let kind = match ty {
            Some("string") => {
                allow(&["format"])?;
                let format = match obj.get("format") {
                    None => None,
                    Some(Value::String(f)) => Some(
                        Format::parse(f)
                            .ok_or_else(|| unsupported(path, format!("format `{f}`")))?,
                    ),
                    Some(_) => return Err(unsupported(path, "non-string format")),
                };
                Kind::String(format)
            }
            Some("integer") => {
                allow(&[])?;
                Kind::Integer
            }
            Some("number") => {
                allow(&[])?;
                Kind::Number
            }
            Some("boolean") => {
                allow(&[])?;
                Kind::Boolean
            }
            Some("array") => {
                allow(&["items"])?;
                let items = obj
                    .get("items")
                    .ok_or_else(|| unsupported(path, "array without `items`"))?;
                Kind::Array(Box::new(Node::from_json(items, &child(path, "items"))?))
            }
            Some("object") => {
                allow(&["properties", "required", "additionalProperties"])?;
                Kind::Object(parse_object(obj, path)?)
            }
            Some(other) => return Err(unsupported(path, format!("type `{other}`"))),
            None => return Err(unsupported(path, "missing `type`")),
        };
        Ok(Node { kind, description })
    }

    /// Render back to JSON Schema. Inverse of [`Node::from_json`] up to key order and the
    /// order of `required` (emitted sorted).
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        match &self.kind {
            Kind::String(format) => {
                out.insert("type".into(), json!("string"));
                if let Some(f) = format {
                    out.insert("format".into(), json!(f.as_str()));
                }
            }
            Kind::Integer => {
                out.insert("type".into(), json!("integer"));
            }
            Kind::Number => {
                out.insert("type".into(), json!("number"));
            }
            Kind::Boolean => {
                out.insert("type".into(), json!("boolean"));
            }
            Kind::Array(items) => {
                out.insert("type".into(), json!("array"));
                out.insert("items".into(), items.to_json());
            }
            Kind::Object(o) => {
                out.insert("type".into(), json!("object"));
                if !o.properties.is_empty() {
                    let props: Map<String, Value> = o
                        .properties
                        .iter()
                        .map(|(k, n)| (k.clone(), n.to_json()))
                        .collect();
                    out.insert("properties".into(), Value::Object(props));
                }
                if !o.required.is_empty() {
                    out.insert("required".into(), json!(o.required));
                }
                if o.closed {
                    out.insert("additionalProperties".into(), json!(false));
                }
            }
            Kind::Enum { values, typed } => {
                if let Some(t) = typed {
                    out.insert("type".into(), json!(t));
                }
                out.insert("enum".into(), Value::Array(values.clone()));
            }
        }
        if let Some(d) = &self.description {
            out.insert("description".into(), json!(d));
        }
        Value::Object(out)
    }
}

fn parse_enum(values: &Value, ty: Option<&str>, path: &str) -> Result<Kind, Error> {
    let values = values
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| unsupported(path, "`enum` is not a non-empty array"))?;
    let typed = match ty {
        None => None,
        Some("string") => Some("string"),
        Some("integer") => Some("integer"),
        Some("number") => Some("number"),
        Some(other) => return Err(unsupported(path, format!("enum of type `{other}`"))),
    };
    let mut seen = BTreeSet::new();
    for v in values {
        let ok = match (typed, v) {
            (Some("string") | None, Value::String(_)) => true,
            (Some("integer"), Value::Number(n)) => n.is_i64() || n.is_u64(),
            (Some("number") | None, Value::Number(_)) => true,
            _ => false,
        };
        if !ok {
            return Err(unsupported(
                path,
                format!("enum literal {v} does not match its type"),
            ));
        }
        if !seen.insert(v.to_string()) {
            return Err(unsupported(path, format!("duplicate enum literal {v}")));
        }
    }
    Ok(Kind::Enum {
        values: values.clone(),
        typed,
    })
}

fn parse_object(obj: &Map<String, Value>, path: &str) -> Result<Object, Error> {
    let mut properties = BTreeMap::new();
    if let Some(props) = obj.get("properties") {
        let props = props
            .as_object()
            .ok_or_else(|| unsupported(path, "`properties` is not an object"))?;
        for (k, v) in props {
            properties.insert(k.clone(), Node::from_json(v, &child(path, k))?);
        }
    }
    let mut required = BTreeSet::new();
    if let Some(req) = obj.get("required") {
        let req = req
            .as_array()
            .ok_or_else(|| unsupported(path, "`required` is not an array"))?;
        for r in req {
            let name = r
                .as_str()
                .ok_or_else(|| unsupported(path, "non-string in `required`"))?;
            if !properties.contains_key(name) {
                return Err(unsupported(
                    path,
                    format!("required `{name}` is not a property"),
                ));
            }
            if !required.insert(name.to_string()) {
                return Err(unsupported(path, format!("`{name}` required twice")));
            }
        }
    }
    let closed = match obj.get("additionalProperties") {
        None => false,
        Some(Value::Bool(false)) => true,
        Some(_) => return Err(unsupported(path, "`additionalProperties` other than false")),
    };
    Ok(Object {
        properties,
        required,
        closed,
    })
}

/// Parse a tool's parameters. `None` means the tool takes no parameters. The top level must
/// be an object, as OpenAI requires.
pub fn normalize_tool(tool: &ToolDef) -> Result<Option<Node>, Error> {
    let Some(params) = &tool.parameters else {
        return Ok(None);
    };
    let path = tool.name.as_str();
    let node = Node::from_json(params, path)?;
    if !matches!(node.kind, Kind::Object(_)) {
        return Err(unsupported(path, "parameters are not an object"));
    }
    Ok(Some(node))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON Schema with `required` sorted: the canonical form two equivalent schemas share.
    fn canon(v: &Value) -> Value {
        match v {
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| {
                        let v = match (k.as_str(), v) {
                            ("required", Value::Array(a)) => {
                                let mut a = a.clone();
                                a.sort_by_key(|x| x.to_string());
                                Value::Array(a)
                            }
                            _ => canon(v),
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            ),
            Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
            other => other.clone(),
        }
    }

    fn calendar() -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })
    }

    fn tool(params: Value) -> ToolDef {
        ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(params),
        }
    }

    #[test]
    fn supported_schema_round_trips_canonically() {
        for schema in [
            calendar(),
            json!({"type": "object", "properties": {
                "passengers": {"type": "object", "properties": {
                    "adults": {"type": "integer"}, "children": {"type": "integer"}},
                    "required": ["adults"], "additionalProperties": false},
                "price": {"type": "number"},
                "ok": {"type": "boolean"},
                "status": {"type": "array", "items": {"type": "string", "enum": ["a", "b"]}},
                "level": {"enum": [1, 2.5]}
            }, "required": ["price", "ok"]}),
            json!({"type": "object"}),
        ] {
            let node = normalize_tool(&tool(schema.clone())).unwrap().unwrap();
            assert_eq!(canon(&node.to_json()), canon(&schema));
        }
    }

    #[test]
    fn no_parameters_is_supported() {
        let t = ToolDef {
            parameters: None,
            ..tool(json!({}))
        };
        assert_eq!(normalize_tool(&t), Ok(None));
    }

    #[test]
    fn unsupported_features_are_rejected_not_dropped() {
        for (schema, needle) in [
            (
                json!({"type": "object", "properties": {"x": {"anyOf": []}}}),
                "anyOf",
            ),
            (
                json!({"type": "object", "properties": {"x": {"$ref": "#/a"}}}),
                "$ref",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "string", "pattern": "a"}}}),
                "pattern",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "integer", "minimum": 0}}}),
                "minimum",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": ["string", "null"]}}}),
                "single string",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "string", "format": "ipv4"}}}),
                "ipv4",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "string", "default": "a"}}}),
                "default",
            ),
            (
                json!({"type": "object", "additionalProperties": true}),
                "additionalProperties",
            ),
            (
                json!({"type": "object", "additionalProperties": {"type": "string"}}),
                "additionalProperties",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "null"}}}),
                "null",
            ),
            (json!({"type": "object", "required": ["ghost"]}), "ghost"),
            (
                json!({"type": "object", "properties": {"x": {"enum": ["a", "a"]}}}),
                "duplicate",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "integer", "enum": [1.5]}}}),
                "does not match",
            ),
            (
                json!({"type": "array", "items": {"type": "string"}}),
                "not an object",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "array"}}}),
                "items",
            ),
            (
                json!({"type": "object", "properties": {"x": {"type": "integer", "items": {}}}}),
                "not valid here",
            ),
        ] {
            let err = normalize_tool(&tool(schema.clone())).unwrap_err();
            assert!(
                err.to_string().contains(needle),
                "{schema}: expected `{needle}` in `{err}`"
            );
            assert_eq!(err.as_label(), "unsupported");
        }
    }
}
