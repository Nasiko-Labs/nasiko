//! The supported JSON Schema subset, as a typed tree.
//!
//! [`build_tool`] turns a native [`ToolDef`] into a [`Tool`] (or refuses with
//! [`EncodeError::Unsupported`] — a whitelist, never a blacklist), and [`to_tooldef`] turns a
//! [`Tool`] back into canonical JSON Schema. The encoder renders a `Tool` as text, the parser
//! reads text back into a `Tool`, and the validator checks call arguments against one.

use std::collections::BTreeSet;

use serde_json::{Map, Number, Value, json};

use crate::error::EncodeError;
use crate::types::{DescriptionPolicy, ToolDef};

/// String formats with a compact keyword.
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

    fn from_schema(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.schema_name() == s)
    }
}

/// A value type in the supported subset.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    Str(Option<Format>),
    Int,
    Num,
    Bool,
    Null,
    Array(Box<Ty>),
    Obj(Vec<Prop>),
    /// Non-empty, duplicate-free, all strings / all integers / all numbers.
    Enum(Vec<Value>),
}

/// One object property.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prop {
    pub key: String,
    pub required: bool,
    pub ty: Ty,
    pub desc: Option<String>,
    pub default: Option<Value>,
    pub min: Option<Number>,
    pub max: Option<Number>,
}

/// One tool in the supported subset. `params` is the root object's properties, in render order
/// (required first in `required`-array order, then optional alphabetically).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tool {
    pub name: String,
    pub desc: Option<String>,
    pub params: Vec<Prop>,
}

/// Every compact type keyword. A bare enum value may not equal one of these.
pub(crate) const TYPE_KEYWORDS: [&str; 13] = [
    "str", "int", "num", "bool", "null", "obj", "datetime", "date", "time", "email", "uri", "uuid",
    "enum",
];

/// Keywords allowed on any schema node (array items, nested values).
const NODE_KEYS: [&str; 7] = [
    "type",
    "enum",
    "format",
    "items",
    "properties",
    "required",
    "additionalProperties",
];
/// Extra keywords allowed on a property (they have a place on the property's line).
const PROP_KEYS: [&str; 4] = ["description", "default", "minimum", "maximum"];

/// Tool names: `[A-Za-z_][A-Za-z0-9_.-]*`, at most 64 chars (no `:` — it ends the name on the
/// header line).
pub(crate) fn is_name(s: &str) -> bool {
    is_word(s, |c| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
    })
}

/// Property keys and bare enum values: `[A-Za-z_][A-Za-z0-9_-]*`, at most 64 chars.
pub(crate) fn is_key(s: &str) -> bool {
    is_word(s, |c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn is_word(s: &str, rest: impl Fn(char) -> bool) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.len() <= 64 && chars.all(rest)
}

/// Whether a string enum value can be rendered bare (`a|b`).
pub(crate) fn is_bare_enum_value(s: &str) -> bool {
    is_key(s) && !TYPE_KEYWORDS.contains(&s)
}

/// Collapse internal whitespace (including newlines) to single spaces; `None` when empty.
pub(crate) fn collapse(s: &str) -> Option<String> {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// The JSON Schema `type` an enum's values imply, or `None` if they are mixed/unsupported.
pub(crate) fn enum_kind(values: &[Value]) -> Option<&'static str> {
    if values.is_empty() {
        return None;
    }
    if values.iter().all(Value::is_string) {
        Some("string")
    } else if values.iter().all(|v| v.is_i64() || v.is_u64()) {
        Some("integer")
    } else if values.iter().all(Value::is_number) {
        Some("number")
    } else {
        None
    }
}

struct Ctx<'a> {
    tool: &'a str,
    policy: DescriptionPolicy,
}

impl Ctx<'_> {
    fn unsupported(&self, path: &str, keyword: &str) -> EncodeError {
        EncodeError::Unsupported {
            tool: self.tool.to_string(),
            path: if path.is_empty() {
                "/".into()
            } else {
                path.into()
            },
            keyword: keyword.to_string(),
        }
    }
}

/// Build the typed tree for one tool, refusing anything outside the supported subset.
pub(crate) fn build_tool(def: &ToolDef, policy: DescriptionPolicy) -> Result<Tool, EncodeError> {
    let ctx = Ctx {
        tool: &def.name,
        policy,
    };
    if !is_name(&def.name) {
        return Err(ctx.unsupported("", "name"));
    }
    let params = match &def.parameters {
        None => Vec::new(),
        Some(Value::Object(root)) => {
            for key in root.keys() {
                if !matches!(
                    key.as_str(),
                    "type" | "properties" | "required" | "additionalProperties"
                ) {
                    return Err(ctx.unsupported("", key));
                }
            }
            match root.get("type") {
                None => {}
                Some(Value::String(t)) if t == "object" => {}
                Some(_) => return Err(ctx.unsupported("", "type")),
            }
            check_additional(&ctx, root, "")?;
            if root.contains_key("properties") || root.contains_key("required") {
                build_object(&ctx, root, "")?
            } else {
                Vec::new()
            }
        }
        Some(_) => return Err(ctx.unsupported("", "parameters")),
    };
    Ok(Tool {
        name: def.name.clone(),
        desc: def.description.as_deref().and_then(collapse),
        params,
    })
}

/// `additionalProperties` is only accepted as `false` (the validator is strict anyway).
fn check_additional(
    ctx: &Ctx<'_>,
    map: &Map<String, Value>,
    path: &str,
) -> Result<(), EncodeError> {
    match map.get("additionalProperties") {
        None | Some(Value::Bool(false)) => Ok(()),
        Some(_) => Err(ctx.unsupported(path, "additionalProperties")),
    }
}

fn build_object(
    ctx: &Ctx<'_>,
    map: &Map<String, Value>,
    path: &str,
) -> Result<Vec<Prop>, EncodeError> {
    // An object with no `properties` is an open dictionary: nothing to render or validate.
    let Some(Value::Object(props)) = map.get("properties") else {
        return Err(ctx.unsupported(path, "properties"));
    };
    let required: Vec<String> = match map.get("required") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                let Some(key) = item.as_str() else {
                    return Err(ctx.unsupported(path, "required"));
                };
                if !props.contains_key(key) || out.iter().any(|k| k == key) {
                    return Err(ctx.unsupported(path, "required"));
                }
                out.push(key.to_string());
            }
            out
        }
        Some(_) => return Err(ctx.unsupported(path, "required")),
    };
    // Required first, in `required` order (arrays keep the author's order), then optional
    // alphabetically (`serde_json` maps are already sorted, the author's order is gone).
    let mut ordered: Vec<(&str, bool)> = required.iter().map(|k| (k.as_str(), true)).collect();
    ordered.extend(
        props
            .keys()
            .filter(|k| !required.contains(k))
            .map(|k| (k.as_str(), false)),
    );
    let mut out = Vec::with_capacity(ordered.len());
    for (key, is_required) in ordered {
        let child_path = format!("{path}/{key}");
        if !is_key(key) {
            return Err(ctx.unsupported(&child_path, "property name"));
        }
        let Some(Value::Object(schema)) = props.get(key) else {
            return Err(ctx.unsupported(&child_path, "property schema"));
        };
        out.push(build_prop(ctx, key, is_required, schema, &child_path)?);
    }
    Ok(out)
}

fn build_prop(
    ctx: &Ctx<'_>,
    key: &str,
    required: bool,
    schema: &Map<String, Value>,
    path: &str,
) -> Result<Prop, EncodeError> {
    for k in schema.keys() {
        if !NODE_KEYS.contains(&k.as_str()) && !PROP_KEYS.contains(&k.as_str()) {
            return Err(ctx.unsupported(path, k));
        }
    }
    let ty = build_ty(ctx, schema, path)?;
    let desc = match schema.get("description") {
        None => None,
        Some(Value::String(d)) => collapse(d),
        Some(_) => return Err(ctx.unsupported(path, "description")),
    };
    let desc = match (desc, ctx.policy) {
        (Some(d), DescriptionPolicy::DropRedundant) if is_redundant(&d, key, ctx.tool) => None,
        (d, _) => d,
    };
    let default = match schema.get("default") {
        None => None,
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null)) => {
            Some(v.clone())
        }
        Some(_) => return Err(ctx.unsupported(path, "default")),
    };
    let bound = |name: &str| -> Result<Option<Number>, EncodeError> {
        match schema.get(name) {
            None => Ok(None),
            Some(Value::Number(n)) if matches!(ty, Ty::Int | Ty::Num) => Ok(Some(n.clone())),
            Some(_) => Err(ctx.unsupported(path, name)),
        }
    };
    let min = bound("minimum")?;
    let max = bound("maximum")?;
    Ok(Prop {
        key: key.to_string(),
        required,
        ty,
        desc,
        default,
        min,
        max,
    })
}

fn build_ty(ctx: &Ctx<'_>, schema: &Map<String, Value>, path: &str) -> Result<Ty, EncodeError> {
    check_additional(ctx, schema, path)?;
    let ty = match schema.get("type") {
        None => None,
        Some(Value::String(t)) => Some(t.as_str()),
        Some(_) => return Err(ctx.unsupported(path, "type")),
    };
    let only_for = |keyword: &str, wanted: &str| -> Result<(), EncodeError> {
        if schema.contains_key(keyword) && ty != Some(wanted) {
            return Err(ctx.unsupported(path, keyword));
        }
        Ok(())
    };
    only_for("items", "array")?;
    only_for("properties", "object")?;
    only_for("required", "object")?;
    only_for("additionalProperties", "object")?;
    only_for("format", "string")?;

    if let Some(values) = schema.get("enum") {
        let Value::Array(values) = values else {
            return Err(ctx.unsupported(path, "enum"));
        };
        let unique: BTreeSet<String> = values.iter().map(Value::to_string).collect();
        let Some(kind) = enum_kind(values) else {
            return Err(ctx.unsupported(path, "enum"));
        };
        if unique.len() != values.len()
            || schema.contains_key("format")
            || ty.is_some_and(|t| t != kind)
        {
            return Err(ctx.unsupported(path, "enum"));
        }
        return Ok(Ty::Enum(values.clone()));
    }

    match ty {
        Some("string") => match schema.get("format") {
            None => Ok(Ty::Str(None)),
            Some(Value::String(f)) => Format::from_schema(f)
                .map(|f| Ty::Str(Some(f)))
                .ok_or_else(|| ctx.unsupported(path, "format")),
            Some(_) => Err(ctx.unsupported(path, "format")),
        },
        Some("integer") => Ok(Ty::Int),
        Some("number") => Ok(Ty::Num),
        Some("boolean") => Ok(Ty::Bool),
        Some("null") => Ok(Ty::Null),
        Some("array") => {
            let Some(Value::Object(items)) = schema.get("items") else {
                return Err(ctx.unsupported(path, "items"));
            };
            let item_path = format!("{path}/items");
            for k in items.keys() {
                if !NODE_KEYS.contains(&k.as_str()) {
                    return Err(ctx.unsupported(&item_path, k));
                }
            }
            Ok(Ty::Array(Box::new(build_ty(ctx, items, &item_path)?)))
        }
        Some("object") => Ok(Ty::Obj(build_object(ctx, schema, path)?)),
        _ => Err(ctx.unsupported(path, "type")),
    }
}

const STOPWORDS: [&str; 12] = [
    "a", "an", "the", "of", "in", "to", "for", "from", "s", "user", "is", "this",
];

/// Whether every non-stopword of `desc` already appears in the property or tool name.
fn is_redundant(desc: &str, key: &str, tool: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let pool: BTreeSet<String> = words(key).into_iter().chain(words(tool)).collect();
    words(desc)
        .iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .all(|w| pool.contains(w))
}

// --------------------------------------------------------------------------
// Typed tree → canonical JSON Schema
// --------------------------------------------------------------------------

/// Canonical JSON Schema for one tool. Every supported schema maps to exactly one canonical
/// form, which is what "schema survived" is compared on (see `README.md`, normalization).
pub(crate) fn to_tooldef(tool: &Tool) -> ToolDef {
    ToolDef {
        name: tool.name.clone(),
        description: tool.desc.clone(),
        parameters: Some(object_schema(&tool.params)),
    }
}

fn object_schema(props: &[Prop]) -> Value {
    let mut properties = Map::new();
    for p in props {
        properties.insert(p.key.clone(), prop_schema(p));
    }
    let mut out = Map::new();
    out.insert("type".into(), json!("object"));
    out.insert("properties".into(), Value::Object(properties));
    let required: Vec<Value> = props
        .iter()
        .filter(|p| p.required)
        .map(|p| Value::String(p.key.clone()))
        .collect();
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    Value::Object(out)
}

fn prop_schema(p: &Prop) -> Value {
    let mut out = match ty_schema(&p.ty) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    if let Some(d) = &p.desc {
        out.insert("description".into(), json!(d));
    }
    if let Some(d) = &p.default {
        out.insert("default".into(), d.clone());
    }
    if let Some(n) = &p.min {
        out.insert("minimum".into(), Value::Number(n.clone()));
    }
    if let Some(n) = &p.max {
        out.insert("maximum".into(), Value::Number(n.clone()));
    }
    Value::Object(out)
}

fn ty_schema(ty: &Ty) -> Value {
    match ty {
        Ty::Str(None) => json!({"type": "string"}),
        Ty::Str(Some(f)) => json!({"type": "string", "format": f.schema_name()}),
        Ty::Int => json!({"type": "integer"}),
        Ty::Num => json!({"type": "number"}),
        Ty::Bool => json!({"type": "boolean"}),
        Ty::Null => json!({"type": "null"}),
        Ty::Array(items) => json!({"type": "array", "items": ty_schema(items)}),
        Ty::Obj(props) => object_schema(props),
        Ty::Enum(values) => json!({"type": enum_kind(values), "enum": values}),
    }
}

/// The canonical form of a native tool under `policy` — what [`crate::decode_tools`] returns
/// for it after [`crate::encode_tools_with`]. Differences from the input are only:
/// whitespace-collapsed descriptions, `required` omitted when empty, enum `type` made explicit,
/// `additionalProperties: false` dropped, absent `parameters` → empty object, and (under
/// `DropRedundant`) dropped redundant parameter descriptions. Key order is irrelevant.
pub fn normalize(def: &ToolDef, policy: DescriptionPolicy) -> Result<ToolDef, EncodeError> {
    build_tool(def, policy).map(|t| to_tooldef(&t))
}
