//! The schema subset the compact form can carry, as a small type tree.
//!
//! Everything goes through this tree: the encoder renders it, the definition parser builds it,
//! and the validator checks arguments against it. So what the model is shown and what its calls
//! are held to cannot drift apart. A schema keyword outside the subset is an error here, never
//! something dropped on the way through.

use serde_json::{Map, Number, Value};

use crate::error::{CompactError, Result};
use crate::text;
use crate::types::ToolDef;

/// Nesting ceiling for both schema conversion and definition parsing, so neither can recurse
/// without bound on hostile input.
pub(crate) const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub ty: Ty,
    /// `type: [T, "null"]`.
    pub nullable: bool,
    pub range: Option<Range>,
    /// Always a scalar.
    pub default: Option<Value>,
    pub description: Option<String>,
}

/// Inclusive bounds: on a number, on the length of a string, or on the item count of an array.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Range {
    pub min: Option<Number>,
    pub max: Option<Number>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    Str {
        format: Option<String>,
    },
    Int,
    Num,
    Bool,
    StrEnum(Vec<String>),
    IntEnum(Vec<i64>),
    NumEnum(Vec<Number>),
    BoolEnum(Vec<bool>),
    Array(Box<Node>),
    Object(Fields),
    /// `{"type":"object"}` with no `properties`: any object.
    AnyObject,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fields {
    pub fields: Vec<Field>,
    /// `additionalProperties: false`.
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub key: String,
    pub required: bool,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tool {
    pub name: String,
    pub description: Option<String>,
    /// `None` when the tool declares no `parameters` at all.
    pub params: Option<Fields>,
}

/// Convert every tool, or fail on the first one that cannot be carried.
pub(crate) fn compile(tools: &[ToolDef]) -> Result<Vec<Tool>> {
    let mut out: Vec<Tool> = Vec::with_capacity(tools.len());
    for def in tools {
        let unsupported = |reason: String| CompactError::Unsupported {
            tool: def.name.clone(),
            reason,
        };
        if !text::is_name(&def.name) {
            return Err(unsupported(
                "name must be non-empty and use only letters, digits, `_`, `-`, `.`".into(),
            ));
        }
        if out.iter().any(|t| t.name == def.name) {
            return Err(unsupported("duplicate tool name".into()));
        }
        let params = match &def.parameters {
            None => None,
            Some(schema) => Some(params_from(schema).map_err(unsupported)?),
        };
        out.push(Tool {
            name: def.name.clone(),
            description: def.description.clone(),
            params,
        });
    }
    Ok(out)
}

pub(crate) fn to_def(tool: &Tool) -> ToolDef {
    ToolDef {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool
            .params
            .as_ref()
            .map(|fields| Value::Object(fields_to_schema(fields))),
    }
}

fn params_from(schema: &Value) -> std::result::Result<Fields, String> {
    let obj = schema
        .as_object()
        .ok_or("`parameters` is not a JSON object")?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err("`parameters` must have `type: object`".into());
    }
    if !obj.contains_key("properties") {
        return Err("`parameters` has no `properties`".into());
    }
    only(
        obj,
        &["type", "properties", "required", "additionalProperties"],
    )?;
    fields_from(obj, 0)
}

fn node_from(schema: &Value, depth: usize) -> std::result::Result<Node, String> {
    if depth > MAX_DEPTH {
        return Err(format!("schema nests deeper than {MAX_DEPTH} levels"));
    }
    let obj = schema.as_object().ok_or("schema is not a JSON object")?;
    let description = match obj.get("description") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("`description` is not a string".into()),
    };
    let (kind, nullable) = match obj.get("type") {
        Some(Value::String(s)) => (s.as_str(), false),
        Some(Value::Array(pair)) => match pair.as_slice() {
            [Value::String(s), Value::String(null)] if null == "null" && s != "null" => {
                (s.as_str(), true)
            }
            _ => return Err("`type` list is not `[T, \"null\"]`".into()),
        },
        Some(_) => return Err("`type` is not a type name".into()),
        // A schema with no `type` is usually a combinator (`oneOf`, `anyOf`, `$ref`); naming it
        // says more than "no type".
        None => {
            only(obj, &["description", "default"])?;
            return Err("schema has no `type`".into());
        }
    };
    let has_enum = obj.contains_key("enum");
    // Keywords any node may carry; each type adds its own below.
    let mut allowed = vec!["type", "description", "default"];
    let ty = match kind {
        "string" if has_enum => {
            allowed.push("enum");
            Ty::StrEnum(enum_values(obj, nullable, |v| {
                v.as_str().map(str::to_string)
            })?)
        }
        "string" => {
            allowed.extend(["format", "minLength", "maxLength"]);
            Ty::Str {
                format: format_from(obj)?,
            }
        }
        "integer" if has_enum => {
            allowed.push("enum");
            Ty::IntEnum(enum_values(obj, nullable, Value::as_i64)?)
        }
        "integer" => {
            allowed.extend(["minimum", "maximum"]);
            Ty::Int
        }
        "number" if has_enum => {
            allowed.push("enum");
            Ty::NumEnum(enum_values(obj, nullable, |v| v.as_number().cloned())?)
        }
        "number" => {
            allowed.extend(["minimum", "maximum"]);
            Ty::Num
        }
        "boolean" if has_enum => {
            allowed.push("enum");
            Ty::BoolEnum(enum_values(obj, nullable, Value::as_bool)?)
        }
        "boolean" => Ty::Bool,
        "array" => {
            allowed.extend(["items", "minItems", "maxItems"]);
            let items = obj.get("items").ok_or("array has no `items`")?;
            Ty::Array(Box::new(node_from(items, depth + 1)?))
        }
        "object" if obj.contains_key("properties") => {
            allowed.extend(["properties", "required", "additionalProperties"]);
            Ty::Object(fields_from(obj, depth)?)
        }
        "object" => Ty::AnyObject,
        other => return Err(format!("unsupported type `{other}`")),
    };
    only(obj, &allowed)?;
    let default = match obj.get("default") {
        None => None,
        Some(Value::Array(_) | Value::Object(_)) => {
            return Err("`default` is not a scalar".into());
        }
        Some(scalar) => Some(scalar.clone()),
    };
    Ok(Node {
        range: range_from(obj, &ty)?,
        ty,
        nullable,
        default,
        description,
    })
}

/// The schema keywords a type's range is written with, and whether they count things (lengths
/// and item counts, which must be whole and non-negative) rather than bound a value.
pub(crate) fn range_keys(ty: &Ty) -> Option<(&'static str, &'static str, bool)> {
    match ty {
        Ty::Str { .. } => Some(("minLength", "maxLength", true)),
        Ty::Int | Ty::Num => Some(("minimum", "maximum", false)),
        Ty::Array(_) => Some(("minItems", "maxItems", true)),
        _ => None,
    }
}

fn range_from(obj: &Map<String, Value>, ty: &Ty) -> std::result::Result<Option<Range>, String> {
    let Some((min_key, max_key, counts)) = range_keys(ty) else {
        return Ok(None);
    };
    let bound = |key: &str| match obj.get(key) {
        None => Ok(None),
        Some(Value::Number(n)) if !counts || n.is_u64() => Ok(Some(n.clone())),
        Some(_) => Err(format!("`{key}` is not a usable number")),
    };
    let (min, max) = (bound(min_key)?, bound(max_key)?);
    Ok((min.is_some() || max.is_some()).then_some(Range { min, max }))
}

fn fields_from(obj: &Map<String, Value>, depth: usize) -> std::result::Result<Fields, String> {
    let properties = obj
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("`properties` is not a JSON object")?;
    let required: Vec<&str> = match obj.get("required") {
        None => Vec::new(),
        Some(Value::Array(names)) => names
            .iter()
            .map(|n| n.as_str().ok_or("`required` holds a non-string"))
            .collect::<std::result::Result<_, _>>()?,
        Some(_) => return Err("`required` is not an array".into()),
    };
    if let Some(stray) = required.iter().find(|r| !properties.contains_key(**r)) {
        return Err(format!("`required` names undeclared property `{stray}`"));
    }
    let closed = match obj.get("additionalProperties") {
        None => false,
        Some(Value::Bool(false)) => true,
        Some(_) => return Err("`additionalProperties` other than `false`".into()),
    };
    if let Some(repeat) = required
        .iter()
        .enumerate()
        .find_map(|(i, name)| required.iter().take(i).any(|r| r == name).then_some(name))
    {
        return Err(format!("`required` names `{repeat}` twice"));
    }
    // Required fields first, in the order `required` lists them: the definition line then leads
    // with what a call must carry, and that order is the one thing about `required` a line can
    // hold, so it is what lets the schema come back exactly.
    let ordered = required
        .iter()
        .filter_map(|name| properties.get_key_value(*name))
        .map(|pair| (pair, true))
        .chain(
            properties
                .iter()
                .filter(|(key, _)| !required.contains(&key.as_str()))
                .map(|pair| (pair, false)),
        );
    let mut fields = Vec::with_capacity(properties.len());
    for ((key, schema), required) in ordered {
        fields.push(Field {
            key: key.clone(),
            required,
            node: node_from(schema, depth + 1).map_err(|e| format!("property `{key}`: {e}"))?,
        });
    }
    Ok(Fields { fields, closed })
}

fn enum_values<T>(
    obj: &Map<String, Value>,
    nullable: bool,
    pick: impl Fn(&Value) -> Option<T>,
) -> std::result::Result<Vec<T>, String> {
    let values = obj
        .get("enum")
        .and_then(Value::as_array)
        .ok_or("`enum` is not an array")?;
    // A nullable enum has to list `null` itself to admit it. Only the last place is accepted,
    // because that is where it is written back.
    let values = match (nullable, values.split_last()) {
        (true, Some((Value::Null, rest))) => rest,
        (true, _) => return Err("nullable `enum` does not end with `null`".into()),
        (false, _) => values.as_slice(),
    };
    if values.is_empty() {
        return Err("`enum` is empty".into());
    }
    values
        .iter()
        .map(|v| pick(v).ok_or_else(|| "`enum` value does not match `type`".to_string()))
        .collect()
}

fn format_from(obj: &Map<String, Value>) -> std::result::Result<Option<String>, String> {
    match obj.get("format") {
        None => Ok(None),
        Some(Value::String(f)) if is_format(f) => Ok(Some(f.clone())),
        Some(_) => Err("`format` is not a simple name".into()),
    }
}

pub(crate) fn is_format(s: &str) -> bool {
    !s.is_empty() && s.chars().all(text::is_ident_char)
}

/// Reject any keyword outside `allowed`. This is what keeps the subset honest: a constraint this
/// crate does not understand stops compaction instead of being silently dropped.
fn only(obj: &Map<String, Value>, allowed: &[&str]) -> std::result::Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(format!("unsupported schema keyword `{key}`")),
        None => Ok(()),
    }
}

fn node_to_schema(node: &Node) -> Value {
    let mut out = match &node.ty {
        Ty::Object(fields) => fields_to_schema(fields),
        _ => Map::new(),
    };
    let mut choices: Option<Vec<Value>> = None;
    let kind = match &node.ty {
        Ty::Str { format } => {
            if let Some(format) = format {
                out.insert("format".into(), Value::String(format.clone()));
            }
            "string"
        }
        Ty::Int => "integer",
        Ty::Num => "number",
        Ty::Bool => "boolean",
        Ty::StrEnum(values) => {
            choices = Some(values.iter().cloned().map(Value::String).collect());
            "string"
        }
        Ty::IntEnum(values) => {
            choices = Some(values.iter().copied().map(Value::from).collect());
            "integer"
        }
        Ty::NumEnum(values) => {
            choices = Some(values.iter().cloned().map(Value::Number).collect());
            "number"
        }
        Ty::BoolEnum(values) => {
            choices = Some(values.iter().copied().map(Value::Bool).collect());
            "boolean"
        }
        Ty::Array(items) => {
            out.insert("items".into(), node_to_schema(items));
            "array"
        }
        Ty::Object(_) | Ty::AnyObject => "object",
    };
    if let Some(mut choices) = choices {
        if node.nullable {
            choices.push(Value::Null);
        }
        out.insert("enum".into(), Value::Array(choices));
    }
    let kind = Value::String(kind.into());
    let kind = if node.nullable {
        Value::Array(vec![kind, Value::String("null".into())])
    } else {
        kind
    };
    out.insert("type".into(), kind);
    if let (Some(range), Some((min_key, max_key, _))) = (&node.range, range_keys(&node.ty)) {
        for (key, bound) in [(min_key, &range.min), (max_key, &range.max)] {
            if let Some(bound) = bound {
                out.insert(key.into(), Value::Number(bound.clone()));
            }
        }
    }
    if let Some(default) = &node.default {
        out.insert("default".into(), default.clone());
    }
    if let Some(description) = &node.description {
        out.insert("description".into(), Value::String(description.clone()));
    }
    Value::Object(out)
}

fn fields_to_schema(fields: &Fields) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("type".into(), Value::String("object".into()));
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &fields.fields {
        properties.insert(field.key.clone(), node_to_schema(&field.node));
        if field.required {
            required.push(Value::String(field.key.clone()));
        }
    }
    out.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    if fields.closed {
        out.insert("additionalProperties".into(), Value::Bool(false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(parameters: Value) -> ToolDef {
        ToolDef {
            name: "t".into(),
            description: None,
            parameters: Some(parameters),
        }
    }

    fn reason(parameters: Value) -> String {
        match compile(&[tool(parameters)]) {
            Err(CompactError::Unsupported { reason, .. }) => reason,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn supported_schema_survives_the_tree() {
        let schema = json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time"},
                "count": {"type": "integer"},
                "ratio": {"type": "number"},
                "flag": {"type": "boolean"},
                "level": {"type": "integer", "enum": [1, 2, 3]},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "tags": {"type": "array", "items": {"type": "string"}},
                "meta": {"type": "object"},
                "owner": {
                    "type": "object",
                    "description": "Who owns it",
                    "properties": {"id": {"type": "integer"}},
                    "required": ["id"],
                    "additionalProperties": false
                }
            },
            "required": ["title", "start"]
        });
        let compiled = compile(&[tool(schema.clone())]).unwrap();
        assert_eq!(to_def(&compiled[0]).parameters.unwrap(), schema);
    }

    #[test]
    fn required_fields_lead_in_the_order_required_lists_them() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "string"},
                "c": {"type": "string"},
                "d": {"type": "string"}
            },
            "required": ["d", "b"]
        });
        let compiled = compile(&[tool(schema.clone())]).unwrap();
        let keys: Vec<&str> = compiled[0]
            .params
            .iter()
            .flat_map(|p| &p.fields)
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(keys, ["d", "b", "a", "c"]);
        assert_eq!(to_def(&compiled[0]).parameters.unwrap(), schema);
    }

    #[test]
    fn unknown_keywords_are_refused_not_dropped() {
        for (schema, keyword) in [
            (json!({"type": "string", "pattern": "^a"}), "pattern"),
            (
                json!({"type": "integer", "exclusiveMinimum": 1}),
                "exclusiveMinimum",
            ),
            (json!({"type": "integer", "multipleOf": 5}), "multipleOf"),
            (json!({"type": "boolean", "minimum": 1}), "minimum"),
            (
                json!({"type": "string", "enum": ["a"], "maxLength": 3}),
                "maxLength",
            ),
            (json!({"type": "string", "const": "x"}), "const"),
            (json!({"type": "string", "title": "T"}), "title"),
            (
                json!({"type": "array", "items": {"type": "string"}, "uniqueItems": true}),
                "uniqueItems",
            ),
        ] {
            let params = json!({"type": "object", "properties": {"p": schema}});
            assert!(
                reason(params).contains(keyword),
                "{keyword} was not refused"
            );
        }
    }

    #[test]
    fn schema_shapes_outside_the_subset_are_refused() {
        for property in [
            json!({"anyOf": [{"type": "string"}, {"type": "integer"}]}),
            json!({"$ref": "#/definitions/x"}),
            json!({"type": ["null", "string"]}),
            json!({"type": ["string", "integer"]}),
            json!({"type": ["string", "null"], "enum": ["a"]}),
            json!({"type": "string", "minLength": -1}),
            json!({"type": "string", "maxLength": 2.5}),
            json!({"type": "string", "default": ["a"]}),
            json!({"type": "integer", "enum": [1.5]}),
            json!({"type": "null"}),
            json!({"type": "array"}),
            json!({"type": "string", "enum": []}),
            json!({"type": "string", "enum": ["a", 1]}),
            json!({"type": "string", "format": "has space"}),
            json!({"type": "object", "properties": {}, "additionalProperties": true}),
            json!({"type": "object", "properties": {}, "required": ["ghost"]}),
            json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a", "a"]}),
        ] {
            let params = json!({"type": "object", "properties": {"p": property}});
            assert!(
                compile(&[tool(params.clone())]).is_err(),
                "accepted {params}"
            );
        }
    }

    #[test]
    fn top_level_parameters_must_be_a_plain_object() {
        assert!(compile(&[tool(json!({"type": "string"}))]).is_err());
        assert!(compile(&[tool(json!({"type": "object"}))]).is_err());
        assert!(compile(&[tool(json!("nope"))]).is_err());
        assert!(compile(&[tool(json!({"type": "object", "properties": {}}))]).is_ok());
    }

    #[test]
    fn bad_and_duplicate_names_are_refused() {
        let named = |name: &str| ToolDef {
            name: name.into(),
            description: None,
            parameters: None,
        };
        assert!(compile(&[named("")]).is_err());
        assert!(compile(&[named("has space")]).is_err());
        assert!(compile(&[named("a"), named("a")]).is_err());
        assert!(compile(&[named("ns.tool-1_x")]).is_ok());
    }

    #[test]
    fn runaway_nesting_is_refused_instead_of_overflowing() {
        let mut schema = json!({"type": "string"});
        for _ in 0..(MAX_DEPTH + 8) {
            schema = json!({"type": "array", "items": schema});
        }
        let params = json!({"type": "object", "properties": {"p": schema}});
        assert!(reason(params).contains("nests deeper"));
    }
}
