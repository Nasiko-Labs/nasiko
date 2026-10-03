//! The supported JSON Schema dialect as a typed AST.
//!
//! [`Node::from_value`] is a strict lowering: every keyword it does not understand, every shape
//! it cannot carry back out byte-for-byte, is an [`UnsupportedSchema`] error rather than a
//! silent approximation. [`Node::to_value`] is the inverse, emitting only the keywords that were
//! present. The renderer, parser and validator never look at raw JSON Schema; they work on this
//! AST, which is what makes "supported" a single list instead of three.
//!
//! [`UnsupportedSchema`]: crate::ToolCompactError::UnsupportedSchema

use serde_json::{Map, Number, Value};

use crate::error::{Result, ToolCompactError};
use crate::lexeme::is_arg_name;
use crate::limits::{
    MAX_DESCRIPTION_BYTES, MAX_ENUM_MEMBERS, MAX_PROPERTIES, MAX_SCHEMA_DEPTH, MAX_SCHEMA_NODES,
};

/// Inert annotations carried verbatim and ignored by the validator.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Annot {
    pub title: Option<String>,
    pub default: Option<Value>,
    pub examples: Option<Value>,
}

impl Annot {
    pub(crate) fn is_empty(&self) -> bool {
        self.title.is_none() && self.default.is_none() && self.examples.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub kind: Kind,
    /// `type: [T, "null"]`. Always false for `Any`; for `Enum` it mirrors a `null` member.
    pub nullable: bool,
    pub description: Option<String>,
    pub annot: Annot,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// `{}` possibly with description/annotations: accepts anything.
    Any,
    Scalar(Scalar),
    Enum {
        base: EnumBase,
        members: Vec<Value>,
    },
    Array {
        items: Option<Box<Node>>,
        min_items: Option<u64>,
        max_items: Option<u64>,
    },
    Object(ObjectSchema),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ScalarBase {
    #[default]
    Str,
    Int,
    Num,
    Bool,
    Null,
}

impl ScalarBase {
    pub(crate) const fn type_name(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Int => "integer",
            Self::Num => "number",
            Self::Bool => "boolean",
            Self::Null => "null",
        }
    }

    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
            Self::Num => "num",
            Self::Bool => "bool",
            Self::Null => "null",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnumBase {
    Str,
    Int,
}

impl EnumBase {
    pub(crate) const fn type_name(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Int => "integer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Scalar {
    pub base: ScalarBase,
    pub format: Option<String>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub exclusive_minimum: Option<Number>,
    pub exclusive_maximum: Option<Number>,
}

impl Scalar {
    pub(crate) fn has_numeric_bounds(&self) -> bool {
        self.minimum.is_some()
            || self.maximum.is_some()
            || self.exclusive_minimum.is_some()
            || self.exclusive_maximum.is_some()
    }

    pub(crate) fn has_exclusive_bounds(&self) -> bool {
        self.exclusive_minimum.is_some() || self.exclusive_maximum.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct ObjectSchema {
    /// `None` when the `properties` key is absent; entries sorted by name.
    pub properties: Option<Vec<(String, Node)>>,
    /// `None` when the `required` key is absent; original order kept.
    pub required: Option<Vec<String>>,
    /// `None` when the `additionalProperties` key is absent.
    pub additional: Option<bool>,
}

impl ObjectSchema {
    pub(crate) fn is_required(&self, name: &str) -> bool {
        self.required
            .as_ref()
            .is_some_and(|r| r.iter().any(|n| n == name))
    }
}

/// Node counter shared by every schema in one catalog.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    pub nodes: usize,
}

struct Lower<'a> {
    tool: &'a str,
    budget: &'a mut Budget,
}

/// Tracks which keys of one schema object have been consumed so leftovers can be reported.
struct Keys<'a> {
    map: &'a Map<String, Value>,
    seen: Vec<&'a str>,
}

impl<'a> Keys<'a> {
    fn new(map: &'a Map<String, Value>) -> Self {
        Self {
            map,
            seen: Vec::new(),
        }
    }

    fn take(&mut self, key: &'static str) -> Option<&'a Value> {
        let v = self.map.get(key)?;
        self.seen.push(key);
        Some(v)
    }

    fn leftover(&self) -> Option<&'a str> {
        let mut keys: Vec<&str> = self
            .map
            .keys()
            .map(String::as_str)
            .filter(|k| !self.seen.contains(k))
            .collect();
        keys.sort_unstable();
        keys.first().copied()
    }
}

impl Lower<'_> {
    fn unsupported(&self, path: &str, reason: impl Into<String>) -> ToolCompactError {
        ToolCompactError::unsupported(self.tool, path, reason)
    }

    fn string(&self, path: &str, key: &str, v: &Value) -> Result<String> {
        match v {
            Value::String(s) if s.len() <= MAX_DESCRIPTION_BYTES => Ok(s.clone()),
            Value::String(_) => Err(ToolCompactError::limit(
                "MAX_DESCRIPTION_BYTES",
                MAX_DESCRIPTION_BYTES,
            )),
            _ => Err(self.unsupported(path, format!("`{key}` must be a string"))),
        }
    }

    fn bounded_value(&self, v: &Value) -> Result<Value> {
        let size = serde_json::to_string(v)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        if size > MAX_DESCRIPTION_BYTES {
            return Err(ToolCompactError::limit(
                "MAX_DESCRIPTION_BYTES",
                MAX_DESCRIPTION_BYTES,
            ));
        }
        Ok(v.clone())
    }

    fn u64(&self, path: &str, key: &str, v: &Value) -> Result<u64> {
        v.as_u64().ok_or_else(|| {
            self.unsupported(path, format!("`{key}` must be a non-negative integer"))
        })
    }

    fn number(&self, path: &str, key: &str, v: &Value) -> Result<Number> {
        match v {
            Value::Number(n) => Ok(n.clone()),
            _ => Err(self.unsupported(path, format!("`{key}` must be a number"))),
        }
    }

    fn node(&mut self, path: &str, value: &Value, depth: usize) -> Result<Node> {
        if depth > MAX_SCHEMA_DEPTH {
            return Err(ToolCompactError::limit(
                "MAX_SCHEMA_DEPTH",
                MAX_SCHEMA_DEPTH,
            ));
        }
        self.budget.nodes += 1;
        if self.budget.nodes > MAX_SCHEMA_NODES {
            return Err(ToolCompactError::limit(
                "MAX_SCHEMA_NODES",
                MAX_SCHEMA_NODES,
            ));
        }
        let Value::Object(map) = value else {
            return Err(self.unsupported(path, "schema must be a JSON object"));
        };
        let mut keys = Keys::new(map);

        let description = keys
            .take("description")
            .map(|v| self.string(path, "description", v))
            .transpose()?;
        let annot = Annot {
            title: keys
                .take("title")
                .map(|v| self.string(path, "title", v))
                .transpose()?,
            default: keys
                .take("default")
                .map(|v| self.bounded_value(v))
                .transpose()?,
            examples: keys
                .take("examples")
                .map(|v| self.bounded_value(v))
                .transpose()?,
        };

        let (base, nullable) = match keys.take("type") {
            None => {
                if let Some(k) = keys.leftover() {
                    return Err(self.unsupported(path, format!("unknown keyword `{k}`")));
                }
                return Ok(Node {
                    kind: Kind::Any,
                    nullable: false,
                    description,
                    annot,
                });
            }
            Some(Value::String(t)) => (t.as_str(), false),
            Some(Value::Array(types)) => match types.as_slice() {
                [Value::String(t), Value::String(n)] if n == "null" && t != "null" => {
                    (t.as_str(), true)
                }
                _ => {
                    return Err(self.unsupported(
                        path,
                        "a type list must be exactly [T, \"null\"] with T a single non-null type",
                    ));
                }
            },
            Some(_) => return Err(self.unsupported(path, "`type` must be a string")),
        };

        let enum_members = keys.take("enum");
        let kind = match base {
            "string" | "integer" if enum_members.is_some() => {
                let members = enum_members.unwrap_or(&Value::Null);
                let base = if base == "string" {
                    EnumBase::Str
                } else {
                    EnumBase::Int
                };
                self.enumeration(path, base, members, nullable)?
            }
            "string" => Kind::Scalar(Scalar {
                base: ScalarBase::Str,
                format: keys
                    .take("format")
                    .map(|v| self.string(path, "format", v))
                    .transpose()?,
                min_length: keys
                    .take("minLength")
                    .map(|v| self.u64(path, "minLength", v))
                    .transpose()?,
                max_length: keys
                    .take("maxLength")
                    .map(|v| self.u64(path, "maxLength", v))
                    .transpose()?,
                ..Scalar::default()
            }),
            "integer" | "number" => Kind::Scalar(Scalar {
                base: if base == "integer" {
                    ScalarBase::Int
                } else {
                    ScalarBase::Num
                },
                minimum: keys
                    .take("minimum")
                    .map(|v| self.number(path, "minimum", v))
                    .transpose()?,
                maximum: keys
                    .take("maximum")
                    .map(|v| self.number(path, "maximum", v))
                    .transpose()?,
                exclusive_minimum: keys
                    .take("exclusiveMinimum")
                    .map(|v| self.number(path, "exclusiveMinimum", v))
                    .transpose()?,
                exclusive_maximum: keys
                    .take("exclusiveMaximum")
                    .map(|v| self.number(path, "exclusiveMaximum", v))
                    .transpose()?,
                ..Scalar::default()
            }),
            "boolean" => Kind::Scalar(Scalar {
                base: ScalarBase::Bool,
                ..Scalar::default()
            }),
            "null" => {
                if nullable {
                    return Err(self.unsupported(path, "[\"null\", \"null\"] is not a type"));
                }
                Kind::Scalar(Scalar {
                    base: ScalarBase::Null,
                    ..Scalar::default()
                })
            }
            "array" => Kind::Array {
                items: match keys.take("items") {
                    None => None,
                    Some(items) => Some(Box::new(self.node(
                        &format!("{path}/items"),
                        items,
                        depth + 1,
                    )?)),
                },
                min_items: keys
                    .take("minItems")
                    .map(|v| self.u64(path, "minItems", v))
                    .transpose()?,
                max_items: keys
                    .take("maxItems")
                    .map(|v| self.u64(path, "maxItems", v))
                    .transpose()?,
            },
            "object" => Kind::Object(self.object(path, &mut keys, depth)?),
            other => {
                return Err(self.unsupported(path, format!("unsupported type `{other}`")));
            }
        };

        if enum_members.is_some() && !matches!(kind, Kind::Enum { .. }) {
            return Err(self.unsupported(
                path,
                format!("`enum` is only supported on string and integer types, not `{base}`"),
            ));
        }
        if let Some(k) = keys.leftover() {
            let reason = if matches!(kind, Kind::Enum { .. }) && is_constraint(k) {
                format!("`enum` cannot be combined with `{k}`")
            } else {
                format!("unknown keyword `{k}`")
            };
            return Err(self.unsupported(path, reason));
        }

        Ok(Node {
            kind,
            nullable,
            description,
            annot,
        })
    }

    fn enumeration(
        &self,
        path: &str,
        base: EnumBase,
        members: &Value,
        nullable: bool,
    ) -> Result<Kind> {
        let Value::Array(members) = members else {
            return Err(self.unsupported(path, "`enum` must be an array"));
        };
        if members.len() > MAX_ENUM_MEMBERS {
            return Err(ToolCompactError::limit(
                "MAX_ENUM_MEMBERS",
                MAX_ENUM_MEMBERS,
            ));
        }
        let mut nulls = 0usize;
        let mut others = 0usize;
        for m in members {
            match (m, base) {
                (Value::Null, _) => nulls += 1,
                (Value::String(s), EnumBase::Str) => {
                    if s.len() > MAX_DESCRIPTION_BYTES {
                        return Err(ToolCompactError::limit(
                            "MAX_DESCRIPTION_BYTES",
                            MAX_DESCRIPTION_BYTES,
                        ));
                    }
                    others += 1;
                }
                (Value::Number(_), EnumBase::Int) => others += 1,
                _ => {
                    return Err(self.unsupported(
                        path,
                        format!("`enum` members must all be of type {}", base.type_name()),
                    ));
                }
            }
        }
        if others == 0 {
            return Err(self.unsupported(path, "`enum` needs at least one non-null member"));
        }
        if nulls > 1 {
            return Err(self.unsupported(path, "`enum` lists null more than once"));
        }
        if (nulls == 1) != nullable {
            return Err(self.unsupported(
                path,
                "a nullable enum must list null as a member and a non-nullable enum must not",
            ));
        }
        Ok(Kind::Enum {
            base,
            members: members.clone(),
        })
    }

    fn object(&mut self, path: &str, keys: &mut Keys<'_>, depth: usize) -> Result<ObjectSchema> {
        let properties = match keys.take("properties") {
            None => None,
            Some(Value::Object(props)) => {
                if props.len() > MAX_PROPERTIES {
                    return Err(ToolCompactError::limit("MAX_PROPERTIES", MAX_PROPERTIES));
                }
                let mut names: Vec<&String> = props.keys().collect();
                names.sort_unstable();
                let mut out = Vec::with_capacity(names.len());
                for name in names {
                    if !is_arg_name(name) {
                        return Err(self.unsupported(
                            path,
                            format!("property name `{name}` cannot be written in the notation"),
                        ));
                    }
                    let child_path = format!("{path}/properties/{name}");
                    let child = props.get(name).unwrap_or(&Value::Null);
                    out.push((name.clone(), self.node(&child_path, child, depth + 1)?));
                }
                Some(out)
            }
            Some(_) => return Err(self.unsupported(path, "`properties` must be an object")),
        };

        let required = match keys.take("required") {
            None => None,
            Some(Value::Array(items)) => {
                let mut out: Vec<String> = Vec::with_capacity(items.len());
                for item in items {
                    let Value::String(name) = item else {
                        return Err(self.unsupported(path, "`required` must list strings"));
                    };
                    let known = properties
                        .as_ref()
                        .is_some_and(|p| p.iter().any(|(n, _)| n == name));
                    if !known {
                        return Err(self.unsupported(
                            path,
                            format!("`required` names `{name}`, which is not a declared property"),
                        ));
                    }
                    if out.contains(name) {
                        return Err(
                            self.unsupported(path, format!("`required` lists `{name}` twice"))
                        );
                    }
                    out.push(name.clone());
                }
                Some(out)
            }
            Some(_) => return Err(self.unsupported(path, "`required` must be an array")),
        };

        let additional = match keys.take("additionalProperties") {
            None => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => {
                return Err(self.unsupported(
                    path,
                    "`additionalProperties` must be a boolean (schema-valued is not supported)",
                ));
            }
        };

        Ok(ObjectSchema {
            properties,
            required,
            additional,
        })
    }
}

fn is_constraint(key: &str) -> bool {
    matches!(
        key,
        "format"
            | "minLength"
            | "maxLength"
            | "minimum"
            | "maximum"
            | "exclusiveMinimum"
            | "exclusiveMaximum"
            | "minItems"
            | "maxItems"
            | "items"
            | "properties"
            | "required"
            | "additionalProperties"
    )
}

impl Node {
    /// Lower one tool's `parameters` schema. `budget` is shared across the catalog.
    pub(crate) fn from_value(tool: &str, value: &Value, budget: &mut Budget) -> Result<Self> {
        let mut lower = Lower { tool, budget };
        let node = lower.node("", value, 0)?;
        match &node.kind {
            Kind::Any => Ok(node),
            Kind::Object(_) if !node.nullable => Ok(node),
            Kind::Object(_) => Err(ToolCompactError::unsupported(
                tool,
                "",
                "the parameters object cannot be nullable",
            )),
            _ => Err(ToolCompactError::unsupported(
                tool,
                "",
                "parameters must be an object schema",
            )),
        }
    }

    /// Exact inverse of [`Node::from_value`] for every node it produces.
    pub(crate) fn to_value(&self) -> Value {
        let mut map = Map::new();
        match &self.kind {
            Kind::Any => {}
            Kind::Scalar(s) => {
                map.insert("type".into(), type_value(s.base.type_name(), self.nullable));
                if let Some(f) = &s.format {
                    map.insert("format".into(), Value::String(f.clone()));
                }
                put_u64(&mut map, "minLength", s.min_length);
                put_u64(&mut map, "maxLength", s.max_length);
                put_num(&mut map, "minimum", &s.minimum);
                put_num(&mut map, "maximum", &s.maximum);
                put_num(&mut map, "exclusiveMinimum", &s.exclusive_minimum);
                put_num(&mut map, "exclusiveMaximum", &s.exclusive_maximum);
            }
            Kind::Enum { base, members } => {
                map.insert("type".into(), type_value(base.type_name(), self.nullable));
                map.insert("enum".into(), Value::Array(members.clone()));
            }
            Kind::Array {
                items,
                min_items,
                max_items,
            } => {
                map.insert("type".into(), type_value("array", self.nullable));
                if let Some(items) = items {
                    map.insert("items".into(), items.to_value());
                }
                put_u64(&mut map, "minItems", *min_items);
                put_u64(&mut map, "maxItems", *max_items);
            }
            Kind::Object(o) => {
                map.insert("type".into(), type_value("object", self.nullable));
                if let Some(props) = &o.properties {
                    let mut pm = Map::new();
                    for (name, node) in props {
                        pm.insert(name.clone(), node.to_value());
                    }
                    map.insert("properties".into(), Value::Object(pm));
                }
                if let Some(required) = &o.required {
                    map.insert(
                        "required".into(),
                        Value::Array(required.iter().cloned().map(Value::String).collect()),
                    );
                }
                if let Some(a) = o.additional {
                    map.insert("additionalProperties".into(), Value::Bool(a));
                }
            }
        }
        if let Some(d) = &self.description {
            map.insert("description".into(), Value::String(d.clone()));
        }
        if let Some(t) = &self.annot.title {
            map.insert("title".into(), Value::String(t.clone()));
        }
        if let Some(d) = &self.annot.default {
            map.insert("default".into(), d.clone());
        }
        if let Some(e) = &self.annot.examples {
            map.insert("examples".into(), e.clone());
        }
        Value::Object(map)
    }
}

fn type_value(name: &str, nullable: bool) -> Value {
    if nullable {
        Value::Array(vec![
            Value::String(name.into()),
            Value::String("null".into()),
        ])
    } else {
        Value::String(name.into())
    }
}

fn put_u64(map: &mut Map<String, Value>, key: &str, v: Option<u64>) {
    if let Some(v) = v {
        map.insert(key.into(), Value::Number(v.into()));
    }
}

fn put_num(map: &mut Map<String, Value>, key: &str, v: &Option<Number>) {
    if let Some(v) = v {
        map.insert(key.into(), Value::Number(v.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lower(v: Value) -> Result<Node> {
        Node::from_value("t", &v, &mut Budget::default())
    }

    #[test]
    fn lowering_then_raising_is_exact_for_a_mixed_schema() {
        let v = json!({
            "type": "object",
            "description": "root",
            "title": "Args",
            "properties": {
                "b": {"type": ["string", "null"], "format": "email", "minLength": 1, "default": "x@y"},
                "a": {"type": "integer", "minimum": 0, "exclusiveMaximum": 10.5},
                "c": {"type": "array", "items": {"type": "string", "enum": ["x", "y"]}, "maxItems": 3},
                "d": {"type": "object", "properties": {}, "required": [], "additionalProperties": false},
                "e": {"type": ["integer", "null"], "enum": [1, 2, null]},
                "f": {}
            },
            "required": ["b", "a"],
            "additionalProperties": true
        });
        let node = lower(v.clone()).unwrap();
        assert_eq!(node.to_value(), v);
    }

    #[test]
    fn unknown_keywords_and_unsupported_shapes_are_declined_with_a_path() {
        let cases = [
            (
                json!({"type": "object", "properties": {"a": {"$ref": "#/x"}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "^a"}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"anyOf": []}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "enum": ["x"], "minLength": 1}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": ["string", "null"], "enum": ["x"]}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "enum": ["x", null]}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "number", "enum": [1]}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": ["null", "string"]}}}),
                "/properties/a",
            ),
            (
                json!({"type": "object", "properties": {"has space": {"type": "string"}}}),
                "",
            ),
            (json!({"type": "object", "required": ["ghost"]}), ""),
            (
                json!({"type": "object", "additionalProperties": {"type": "string"}}),
                "",
            ),
            (json!({"type": "string"}), ""),
            (json!({"type": ["object", "null"]}), ""),
            (
                json!({"type": "object", "properties": {"a": {"minimum": 1}}}),
                "/properties/a",
            ),
        ];
        for (schema, path) in cases {
            match lower(schema.clone()) {
                Err(ToolCompactError::UnsupportedSchema { path: p, .. }) => {
                    assert_eq!(p, path, "path for {schema}");
                }
                other => panic!("{schema} should be unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn the_empty_schema_is_any_and_only_metadata_may_accompany_it() {
        assert!(matches!(lower(json!({})).unwrap().kind, Kind::Any));
        assert!(matches!(
            lower(json!({"description": "d", "title": "t"}))
                .unwrap()
                .kind,
            Kind::Any
        ));
        assert!(lower(json!({"minimum": 1})).is_err());
    }

    #[test]
    fn depth_and_node_limits_abort_lowering() {
        let mut deep = json!({"type": "string"});
        for _ in 0..40 {
            deep = json!({"type": "array", "items": deep});
        }
        let root = json!({"type": "object", "properties": {"a": deep}});
        assert!(matches!(
            lower(root),
            Err(ToolCompactError::LimitExceeded {
                limit: "MAX_SCHEMA_DEPTH",
                ..
            })
        ));

        let mut props = Map::new();
        for i in 0..MAX_PROPERTIES {
            props.insert(format!("p{i}"), json!({"type": "string"}));
        }
        let wide = json!({"type": "object", "properties": props});
        let mut budget = Budget::default();
        let mut count = 0;
        let err = loop {
            match Node::from_value("t", &wide, &mut budget) {
                Ok(_) => count += 1,
                Err(e) => break e,
            }
            assert!(count < 100, "node budget never hit");
        };
        assert!(matches!(
            err,
            ToolCompactError::LimitExceeded {
                limit: "MAX_SCHEMA_NODES",
                ..
            }
        ));
    }
}
