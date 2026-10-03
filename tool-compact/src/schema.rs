//! The schema subset the compact form can carry, as a small type tree.
//!
//! Everything goes through this tree: the encoder renders it, the definition parser builds it,
//! and the validator checks arguments against it. So what the model is shown and what its calls
//! are held to cannot drift apart. A schema keyword outside the subset is an error here, never
//! something dropped on the way through.

use serde_json::{Map, Value};

use crate::error::{CompactError, Result};
use crate::text;
use crate::types::ToolDef;

/// Nesting ceiling for both schema conversion and definition parsing, so neither can recurse
/// without bound on hostile input.
pub(crate) const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub ty: Ty,
    pub description: Option<String>,
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
        parameters: tool.params.as_ref().map(fields_to_schema),
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
    let kind = match obj.get("type") {
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err("`type` is not a single type name".into()),
        None => return Err("schema has no `type`".into()),
    };
    let ty = match kind {
        "string" => match obj.get("enum") {
            Some(values) => {
                only(obj, &["type", "description", "enum"])?;
                Ty::StrEnum(enum_values(values, |v| v.as_str().map(str::to_string))?)
            }
            None => {
                only(obj, &["type", "description", "format"])?;
                Ty::Str {
                    format: format_from(obj)?,
                }
            }
        },
        "integer" => match obj.get("enum") {
            Some(values) => {
                only(obj, &["type", "description", "enum"])?;
                Ty::IntEnum(enum_values(values, Value::as_i64)?)
            }
            None => {
                only(obj, &["type", "description"])?;
                Ty::Int
            }
        },
        "number" => {
            only(obj, &["type", "description"])?;
            Ty::Num
        }
        "boolean" => {
            only(obj, &["type", "description"])?;
            Ty::Bool
        }
        "array" => {
            only(obj, &["type", "description", "items"])?;
            let items = obj.get("items").ok_or("array has no `items`")?;
            Ty::Array(Box::new(node_from(items, depth + 1)?))
        }
        "object" if obj.contains_key("properties") => {
            only(
                obj,
                &[
                    "type",
                    "description",
                    "properties",
                    "required",
                    "additionalProperties",
                ],
            )?;
            Ty::Object(fields_from(obj, depth)?)
        }
        "object" => {
            only(obj, &["type", "description"])?;
            Ty::AnyObject
        }
        other => return Err(format!("unsupported type `{other}`")),
    };
    Ok(Node { ty, description })
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
    values: &Value,
    pick: impl Fn(&Value) -> Option<T>,
) -> std::result::Result<Vec<T>, String> {
    let values = values.as_array().ok_or("`enum` is not an array")?;
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
    let mut out = Map::new();
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
            let values = values.iter().cloned().map(Value::String).collect();
            out.insert("enum".into(), Value::Array(values));
            "string"
        }
        Ty::IntEnum(values) => {
            let values = values.iter().copied().map(Value::from).collect();
            out.insert("enum".into(), Value::Array(values));
            "integer"
        }
        Ty::Array(items) => {
            out.insert("items".into(), node_to_schema(items));
            "array"
        }
        Ty::Object(fields) => {
            let mut schema = fields_to_schema(fields);
            if let (Some(map), Some(description)) = (schema.as_object_mut(), &node.description) {
                map.insert("description".into(), Value::String(description.clone()));
            }
            return schema;
        }
        Ty::AnyObject => "object",
    };
    out.insert("type".into(), Value::String(kind.into()));
    if let Some(description) = &node.description {
        out.insert("description".into(), Value::String(description.clone()));
    }
    Value::Object(out)
}

fn fields_to_schema(fields: &Fields) -> Value {
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
    Value::Object(out)
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
            (json!({"type": "integer", "minimum": 1}), "minimum"),
            (json!({"type": "string", "default": "x"}), "default"),
            (json!({"type": "string", "title": "T"}), "title"),
            (
                json!({"type": "array", "items": {"type": "string"}, "minItems": 1}),
                "minItems",
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
            json!({"type": ["string", "null"]}),
            json!({"type": "null"}),
            json!({"type": "array"}),
            json!({"type": "string", "enum": []}),
            json!({"type": "string", "enum": ["a", 1]}),
            json!({"type": "number", "enum": [1.5]}),
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
