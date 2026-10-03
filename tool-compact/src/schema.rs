//! The supported JSON Schema subset, as a typed tree.
//!
//! [`Node::from_json`] is the single gate between arbitrary JSON Schema and the compact
//! format: every keyword is checked against an allowlist, and anything it cannot carry
//! losslessly is reported as [`Unsupported`] so the caller bypasses compaction. The renderer
//! ([`crate::encode`]), the definitions parser ([`crate::defs`]) and the validator
//! ([`crate::validate`]) all work on this tree, never on raw JSON, so they cannot disagree
//! about what a schema means.

use serde_json::{Map, Number, Value};

/// Deepest nesting accepted. Deeper schemas bypass compaction rather than recurse unbounded.
pub(crate) const MAX_DEPTH: usize = 16;

/// A scalar JSON type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Base {
    Str,
    Int,
    Num,
    Bool,
    Null,
}

impl Base {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            Self::Str => "str",
            Self::Int => "int",
            Self::Num => "num",
            Self::Bool => "bool",
            Self::Null => "null",
        }
    }

    pub(crate) fn from_keyword(s: &str) -> Option<Self> {
        Some(match s {
            "str" => Self::Str,
            "int" => Self::Int,
            "num" => Self::Num,
            "bool" => Self::Bool,
            "null" => Self::Null,
            _ => return None,
        })
    }

    pub(crate) fn json_type(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Int => "integer",
            Self::Num => "number",
            Self::Bool => "boolean",
            Self::Null => "null",
        }
    }

    fn from_json_type(s: &str) -> Option<Self> {
        Some(match s {
            "string" => Self::Str,
            "integer" => Self::Int,
            "number" => Self::Num,
            "boolean" => Self::Bool,
            "null" => Self::Null,
            _ => return None,
        })
    }
}

/// Validation keywords and annotations that ride along with any node, rendered inside `(...)`.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Ann {
    pub format: Option<String>,
    pub minimum: Option<Number>,
    pub maximum: Option<Number>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub pattern: Option<String>,
    /// Scalar defaults only. Informational: the decoder never fills a default in.
    pub default: Option<Value>,
}

impl Ann {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// No type constraint (`{}`).
    Any,
    /// One type, or a union of scalar types (`"type": ["string", "null"]`).
    Types(Vec<Base>),
    /// `enum` of scalar literals (strings, integers, booleans, null).
    Enum(Vec<Value>),
    /// `None` = no `items` constraint.
    Array(Option<Box<Node>>),
    Object(Obj),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Obj {
    pub props: Vec<Prop>,
    /// `additionalProperties: false`.
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prop {
    pub name: String,
    pub required: bool,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub kind: Kind,
    pub description: Option<String>,
    pub ann: Ann,
}

/// Why a schema cannot be compacted: where, and which feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unsupported {
    pub path: String,
    pub feature: String,
}

fn unsupported(path: &str, feature: impl Into<String>) -> Unsupported {
    Unsupported {
        path: path.to_string(),
        feature: feature.into(),
    }
}

/// Keywords dropped without changing meaning. Pure annotations: the decoder does not consult
/// them and models do not need them to fill arguments in. Documented in the crate README.
const DROPPED: &[&str] = &["title", "$schema"];

/// Keywords the compact format carries.
const SUPPORTED: &[&str] = &[
    "type",
    "description",
    "enum",
    "format",
    "properties",
    "required",
    "items",
    "additionalProperties",
    "default",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "pattern",
];

/// Property names the line grammar can carry unquoted.
pub(crate) fn is_valid_prop_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$' | '@'))
}

/// Format names the grammar can carry as a bare word.
fn is_valid_format(f: &str) -> bool {
    !f.is_empty()
        && !matches!(f, "closed")
        && f.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Collapse runs of whitespace (including newlines) to one space and trim. The one rewrite
/// applied to descriptions; it keeps the definitions one-line-per-field.
pub(crate) fn normalize_description(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// A literal the grammar can carry in an enum or `default=`.
fn is_scalar_literal(v: &Value) -> bool {
    match v {
        Value::String(_) | Value::Bool(_) | Value::Null => true,
        // Integers only: `1.0` and `1` are different `Value`s, and a float literal would not
        // survive a render/parse round trip byte for byte.
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

fn literal_base(v: &Value) -> Option<Base> {
    match v {
        Value::String(_) => Some(Base::Str),
        Value::Bool(_) => Some(Base::Bool),
        Value::Null => Some(Base::Null),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(Base::Int),
        _ => None,
    }
}

/// The single type every literal shares, if they share one (null excluded).
pub(crate) fn implied_enum_type(values: &[Value]) -> Option<Base> {
    let first = literal_base(values.first()?)?;
    if first == Base::Null {
        return None;
    }
    values
        .iter()
        .all(|v| literal_base(v) == Some(first))
        .then_some(first)
}

fn non_negative_int(v: &Value, path: &str, key: &str) -> Result<u64, Unsupported> {
    v.as_u64()
        .ok_or_else(|| unsupported(path, format!("{key} must be a non-negative integer")))
}

impl Node {
    /// Parse a JSON Schema node. `path` is a JSON-pointer-ish location for error messages.
    pub(crate) fn from_json(v: &Value, path: &str, depth: usize) -> Result<Self, Unsupported> {
        if depth > MAX_DEPTH {
            return Err(unsupported(
                path,
                format!("nesting deeper than {MAX_DEPTH}"),
            ));
        }
        let Value::Object(map) = v else {
            return Err(unsupported(path, "schema is not an object"));
        };
        for key in map.keys() {
            if !SUPPORTED.contains(&key.as_str()) && !DROPPED.contains(&key.as_str()) {
                return Err(unsupported(path, format!("keyword '{key}'")));
            }
        }

        let description = match map.get("description") {
            None => None,
            Some(Value::String(s)) => normalize_description(s),
            Some(_) => return Err(unsupported(path, "non-string description")),
        };
        let ann = Self::parse_ann(map, path)?;

        let kind = if let Some(values) = map.get("enum") {
            Self::parse_enum(map, values, path)?
        } else {
            match map.get("type") {
                Some(Value::String(t)) => Self::parse_single_type(map, t, path, depth)?,
                Some(Value::Array(ts)) => Self::parse_type_union(map, ts, path)?,
                Some(_) => return Err(unsupported(path, "type is not a string or array")),
                // Untyped but structured: treated as the implied type, and `decode_tools`
                // writes that type back out (a documented normalization).
                None if map.contains_key("properties") => Self::parse_object(map, path, depth)?,
                None if map.contains_key("items") => Self::parse_array(map, path, depth)?,
                None => {
                    Self::reject_structural(map, path)?;
                    Kind::Any
                }
            }
        };

        Ok(Self {
            kind,
            description,
            ann,
        })
    }

    /// `properties` / `required` / `items` / `additionalProperties` on a node that is not an
    /// object or array would be silently ignored by the renderer, so they are refused instead.
    fn reject_structural(map: &Map<String, Value>, path: &str) -> Result<(), Unsupported> {
        for key in ["properties", "required", "items", "additionalProperties"] {
            if map.contains_key(key) {
                return Err(unsupported(
                    path,
                    format!("'{key}' on a non-container type"),
                ));
            }
        }
        Ok(())
    }

    fn parse_single_type(
        map: &Map<String, Value>,
        t: &str,
        path: &str,
        depth: usize,
    ) -> Result<Kind, Unsupported> {
        match t {
            "object" => Self::parse_object(map, path, depth),
            "array" => Self::parse_array(map, path, depth),
            other => {
                let base = Base::from_json_type(other)
                    .ok_or_else(|| unsupported(path, format!("type '{other}'")))?;
                Self::reject_structural(map, path)?;
                Ok(Kind::Types(vec![base]))
            }
        }
    }

    fn parse_type_union(
        map: &Map<String, Value>,
        ts: &[Value],
        path: &str,
    ) -> Result<Kind, Unsupported> {
        let mut bases = Vec::new();
        for t in ts {
            let base = t
                .as_str()
                .and_then(Base::from_json_type)
                .ok_or_else(|| unsupported(path, "type union with a non-scalar member"))?;
            if bases.contains(&base) {
                return Err(unsupported(path, "duplicate type in union"));
            }
            bases.push(base);
        }
        if bases.is_empty() {
            return Err(unsupported(path, "empty type union"));
        }
        Self::reject_structural(map, path)?;
        Ok(Kind::Types(bases))
    }

    fn parse_enum(
        map: &Map<String, Value>,
        values: &Value,
        path: &str,
    ) -> Result<Kind, Unsupported> {
        let Value::Array(values) = values else {
            return Err(unsupported(path, "enum is not an array"));
        };
        if values.is_empty() {
            return Err(unsupported(path, "empty enum"));
        }
        if !values.iter().all(is_scalar_literal) {
            return Err(unsupported(path, "enum with non-scalar or float values"));
        }
        for (i, v) in values.iter().enumerate() {
            if values[..i].contains(v) {
                return Err(unsupported(path, "duplicate enum value"));
            }
        }
        // The type is written back from the literals, so it must be exactly the one they imply.
        match map.get("type") {
            None => {}
            Some(Value::String(t)) => {
                let declared = Base::from_json_type(t);
                if declared.is_none() || declared != implied_enum_type(values) {
                    return Err(unsupported(
                        path,
                        format!("enum values do not match type '{t}'"),
                    ));
                }
            }
            Some(_) => return Err(unsupported(path, "enum with a type union")),
        }
        Self::reject_structural(map, path)?;
        Ok(Kind::Enum(values.clone()))
    }

    fn parse_object(
        map: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Kind, Unsupported> {
        if map.contains_key("items") {
            return Err(unsupported(path, "'items' on an object"));
        }
        let closed = match map.get("additionalProperties") {
            None => false,
            Some(Value::Bool(false)) => true,
            // `true` or a schema: extra keys would be legal, but the decoder rejects every
            // undeclared key. Bypass rather than reject calls the schema allows.
            Some(_) => return Err(unsupported(path, "additionalProperties other than false")),
        };
        let required: Vec<&str> = match map.get("required") {
            None => Vec::new(),
            Some(Value::Array(r)) => r
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| unsupported(path, "non-string in required"))
                })
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(unsupported(path, "required is not an array")),
        };
        let props_map = match map.get("properties") {
            None => Map::new(),
            Some(Value::Object(p)) => p.clone(),
            Some(_) => return Err(unsupported(path, "properties is not an object")),
        };
        for r in &required {
            if !props_map.contains_key(*r) {
                return Err(unsupported(
                    path,
                    format!("required '{r}' is not a property"),
                ));
            }
        }
        for (i, r) in required.iter().enumerate() {
            if required[..i].contains(r) {
                return Err(unsupported(path, format!("duplicate required '{r}'")));
            }
        }
        // Required fields first, in `required` order, then the optional ones. The order of
        // `required` then survives a render/parse round trip exactly, and the fields a model
        // must not omit come first.
        let ordered = required.iter().copied().chain(
            props_map
                .keys()
                .map(String::as_str)
                .filter(|k| !required.contains(k)),
        );
        let mut props = Vec::with_capacity(props_map.len());
        for name in ordered {
            let schema = &props_map[name];
            let child_path = format!("{path}/properties/{name}");
            if !is_valid_prop_name(name) {
                return Err(unsupported(&child_path, "property name needs quoting"));
            }
            props.push(Prop {
                name: name.to_string(),
                required: required.contains(&name),
                node: Self::from_json(schema, &child_path, depth + 1)?,
            });
        }
        Ok(Kind::Object(Obj { props, closed }))
    }

    fn parse_array(
        map: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Result<Kind, Unsupported> {
        for key in ["properties", "required", "additionalProperties"] {
            if map.contains_key(key) {
                return Err(unsupported(path, format!("'{key}' on an array")));
            }
        }
        let Some(items) = map.get("items") else {
            return Ok(Kind::Array(None));
        };
        let item_path = format!("{path}/items");
        let item = Self::from_json(items, &item_path, depth + 1)?;
        // An item has no field line of its own to carry a description on.
        if item.description.is_some() {
            return Err(unsupported(&item_path, "description on array items"));
        }
        // `items: {}` constrains nothing; it is the same array as one with no `items`.
        if item.kind == Kind::Any && item.ann.is_empty() {
            return Ok(Kind::Array(None));
        }
        Ok(Kind::Array(Some(Box::new(item))))
    }

    fn parse_ann(map: &Map<String, Value>, path: &str) -> Result<Ann, Unsupported> {
        let mut ann = Ann::default();
        if let Some(f) = map.get("format") {
            match f.as_str() {
                Some(f) if is_valid_format(f) => ann.format = Some(f.to_string()),
                _ => return Err(unsupported(path, "format is not a plain word")),
            }
        }
        for (key, slot) in [("minimum", &mut ann.minimum), ("maximum", &mut ann.maximum)] {
            if let Some(v) = map.get(key) {
                match v {
                    Value::Number(n) => *slot = Some(n.clone()),
                    _ => return Err(unsupported(path, format!("{key} is not a number"))),
                }
            }
        }
        for (key, slot) in [
            ("minLength", &mut ann.min_length),
            ("maxLength", &mut ann.max_length),
            ("minItems", &mut ann.min_items),
            ("maxItems", &mut ann.max_items),
        ] {
            if let Some(v) = map.get(key) {
                *slot = Some(non_negative_int(v, path, key)?);
            }
        }
        if let Some(p) = map.get("pattern") {
            let Some(p) = p.as_str() else {
                return Err(unsupported(path, "pattern is not a string"));
            };
            // ECMA-262 regexes the Rust engine cannot compile (lookaround, backreferences)
            // would leave the decoder unable to validate — bypass instead.
            if regex::Regex::new(p).is_err() {
                return Err(unsupported(
                    path,
                    "pattern not supported by the regex engine",
                ));
            }
            ann.pattern = Some(p.to_string());
        }
        if let Some(d) = map.get("default") {
            if !is_scalar_literal(d) {
                return Err(unsupported(path, "non-scalar or float default"));
            }
            ann.default = Some(d.clone());
        }
        Ok(ann)
    }

    /// Serialize back to JSON Schema (used by `decode_tools`).
    pub(crate) fn to_json(&self) -> Value {
        let mut m = Map::new();
        match &self.kind {
            Kind::Any => {}
            Kind::Types(bases) => {
                let t = if let [one] = bases.as_slice() {
                    Value::String(one.json_type().into())
                } else {
                    Value::Array(
                        bases
                            .iter()
                            .map(|b| Value::String(b.json_type().into()))
                            .collect(),
                    )
                };
                m.insert("type".into(), t);
            }
            Kind::Enum(values) => {
                if let Some(base) = implied_enum_type(values) {
                    m.insert("type".into(), Value::String(base.json_type().into()));
                }
                m.insert("enum".into(), Value::Array(values.clone()));
            }
            Kind::Array(item) => {
                m.insert("type".into(), "array".into());
                if let Some(item) = item {
                    m.insert("items".into(), item.to_json());
                }
            }
            Kind::Object(obj) => {
                m.insert("type".into(), "object".into());
                let mut props = Map::new();
                let mut required = Vec::new();
                for p in &obj.props {
                    props.insert(p.name.clone(), p.node.to_json());
                    if p.required {
                        required.push(Value::String(p.name.clone()));
                    }
                }
                m.insert("properties".into(), Value::Object(props));
                if !required.is_empty() {
                    m.insert("required".into(), Value::Array(required));
                }
                if obj.closed {
                    m.insert("additionalProperties".into(), Value::Bool(false));
                }
            }
        }
        if let Some(d) = &self.description {
            m.insert("description".into(), Value::String(d.clone()));
        }
        let a = &self.ann;
        if let Some(f) = &a.format {
            m.insert("format".into(), Value::String(f.clone()));
        }
        if let Some(n) = &a.minimum {
            m.insert("minimum".into(), Value::Number(n.clone()));
        }
        if let Some(n) = &a.maximum {
            m.insert("maximum".into(), Value::Number(n.clone()));
        }
        for (key, v) in [
            ("minLength", a.min_length),
            ("maxLength", a.max_length),
            ("minItems", a.min_items),
            ("maxItems", a.max_items),
        ] {
            if let Some(v) = v {
                m.insert(key.into(), Value::from(v));
            }
        }
        if let Some(p) = &a.pattern {
            m.insert("pattern".into(), Value::String(p.clone()));
        }
        if let Some(d) = &a.default {
            m.insert("default".into(), d.clone());
        }
        Value::Object(m)
    }

    /// Every `pattern` in this subtree, for the decoder to compile once up front.
    pub(crate) fn patterns<'a>(&'a self, out: &mut Vec<&'a str>) {
        if let Some(p) = &self.ann.pattern {
            out.push(p);
        }
        match &self.kind {
            Kind::Array(Some(item)) => item.patterns(out),
            Kind::Object(obj) => obj.props.iter().for_each(|p| p.node.patterns(out)),
            _ => {}
        }
    }
}
