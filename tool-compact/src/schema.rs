//! JSON Schema → compact [`TypeExpr`] / [`ParamSpec`].
//!
//! Unsupported features return [`CompactError::UnsupportedSchema`] — never silently
//! approximated.

use serde_json::{Map, Value};

use crate::error::{CompactError, Result};
use crate::types::{ParamSpec, TypeExpr};

/// Keywords that mean we cannot faithfully compact the schema.
///
/// Includes constraint keywords we do not encode or enforce — fail closed rather than
/// silently dropping rules private evaluation may rely on.
const UNSUPPORTED_KEYS: &[&str] = &[
    "anyOf",
    "oneOf",
    "allOf",
    "not",
    "$ref",
    "$defs",
    "definitions",
    "patternProperties",
    "dependentSchemas",
    "dependentRequired",
    "if",
    "then",
    "else",
    "unevaluatedProperties",
    "unevaluatedItems",
    "prefixItems",
    "contains",
    "propertyNames",
    // Constraints we neither encode nor validate — bypass compaction.
    "pattern",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minItems",
    "maxItems",
    "uniqueItems",
    "const",
    "minProperties",
    "maxProperties",
];

/// Parse a tool's `parameters` JSON Schema into ordered [`ParamSpec`]s.
pub(crate) fn parse_parameters(parameters: Option<&Value>) -> Result<Vec<ParamSpec>> {
    let Some(schema) = parameters else {
        return Ok(Vec::new());
    };
    if schema.is_null() {
        return Ok(Vec::new());
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| CompactError::InvalidSchema("parameters must be a JSON object".into()))?;

    reject_unsupported(obj, "parameters")?;
    reject_additional_properties_schema(obj, "parameters")?;

    // Empty / missing type treated as object when properties present.
    let ty = obj.get("type").and_then(|t| t.as_str()).unwrap_or("object");
    if ty != "object" {
        return Err(CompactError::UnsupportedSchema(format!(
            "top-level parameters type must be object, got {ty}"
        )));
    }

    parse_object_fields(obj)
}

/// `additionalProperties` as a schema object cannot be represented compactly.
fn reject_additional_properties_schema(obj: &Map<String, Value>, path: &str) -> Result<()> {
    match obj.get("additionalProperties") {
        Some(Value::Object(_)) => Err(CompactError::UnsupportedSchema(format!(
            "{path}: additionalProperties schema objects are not supported for compaction"
        ))),
        _ => Ok(()),
    }
}

fn parse_object_fields(obj: &Map<String, Value>) -> Result<Vec<ParamSpec>> {
    reject_unsupported(obj, "object")?;
    reject_additional_properties_schema(obj, "object")?;

    let required: Vec<String> = obj
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let Some(props) = obj.get("properties").and_then(|p| p.as_object()) else {
        // Bare `type: object` with no properties — opaque.
        return Ok(Vec::new());
    };

    // Deterministic: sort property names (JSON object order is not reliable across parsers).
    let mut names: Vec<&String> = props.keys().collect();
    names.sort();

    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let prop = &props[name];
        let prop_obj = prop.as_object().ok_or_else(|| {
            CompactError::InvalidSchema(format!("property '{name}' schema must be an object"))
        })?;
        reject_unsupported(prop_obj, name)?;

        let type_expr = parse_type_expr(prop_obj, name)?;
        let description = prop_obj
            .get("description")
            .and_then(|d| d.as_str())
            .map(str::to_string);
        out.push(ParamSpec {
            name: name.clone(),
            required: required.iter().any(|r| r == name),
            type_expr,
            description,
        });
    }
    Ok(out)
}

fn parse_type_expr(obj: &Map<String, Value>, path: &str) -> Result<TypeExpr> {
    if let Some(enum_vals) = obj.get("enum") {
        let arr = enum_vals
            .as_array()
            .ok_or_else(|| CompactError::InvalidSchema(format!("{path}: enum must be an array")))?;
        let mut variants = Vec::with_capacity(arr.len());
        for v in arr {
            let s = v.as_str().ok_or_else(|| {
                CompactError::UnsupportedSchema(format!("{path}: only string enums are supported"))
            })?;
            if s.contains('|') || s.contains(',') || s.contains(' ') || s.is_empty() {
                return Err(CompactError::UnsupportedSchema(format!(
                    "{path}: enum variant {s:?} cannot be represented compactly"
                )));
            }
            variants.push(s.to_string());
        }
        if variants.is_empty() {
            return Err(CompactError::InvalidSchema(format!("{path}: empty enum")));
        }
        return Ok(TypeExpr::Enum(variants));
    }

    // Nullable union: ["string","null"] → treat as the non-null type (optional at field level).
    let type_val = obj.get("type");
    let type_str = match type_val {
        None => {
            // No type but has properties → object.
            if obj.contains_key("properties") {
                "object"
            } else {
                return Err(CompactError::UnsupportedSchema(format!(
                    "{path}: missing type"
                )));
            }
        }
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Array(arr)) => {
            let mut non_null: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            non_null.retain(|t| *t != "null");
            if non_null.len() != 1 {
                return Err(CompactError::UnsupportedSchema(format!(
                    "{path}: multi-type unions are unsupported"
                )));
            }
            non_null[0]
        }
        Some(_) => {
            return Err(CompactError::InvalidSchema(format!(
                "{path}: type must be a string or array"
            )));
        }
    };

    match type_str {
        "string" => Ok(match obj.get("format").and_then(|f| f.as_str()) {
            Some("date-time") => TypeExpr::Datetime,
            Some("date") => TypeExpr::Date,
            Some("time") => TypeExpr::Time,
            Some("uri" | "url" | "uri-reference") => TypeExpr::Uri,
            _ => TypeExpr::Str,
        }),
        "integer" => Ok(TypeExpr::Int),
        "number" => Ok(TypeExpr::Num),
        "boolean" => Ok(TypeExpr::Bool),
        "null" => Ok(TypeExpr::Null),
        "array" => {
            let items = obj.get("items").ok_or_else(|| {
                CompactError::UnsupportedSchema(format!("{path}: array without items"))
            })?;
            let items_obj = items.as_object().ok_or_else(|| {
                CompactError::UnsupportedSchema(format!(
                    "{path}: array items must be a single schema object"
                ))
            })?;
            reject_unsupported(items_obj, &format!("{path}.items"))?;
            let inner = parse_type_expr(items_obj, &format!("{path}.items"))?;
            Ok(TypeExpr::Array(Box::new(inner)))
        }
        "object" => {
            if !obj.contains_key("properties") {
                return Ok(TypeExpr::OpaqueObject);
            }
            let fields = parse_object_fields(obj)?;
            Ok(TypeExpr::Object(fields))
        }
        other => Err(CompactError::UnsupportedSchema(format!(
            "{path}: unsupported type {other}"
        ))),
    }
}

fn reject_unsupported(obj: &Map<String, Value>, path: &str) -> Result<()> {
    for key in UNSUPPORTED_KEYS {
        if obj.contains_key(*key) {
            return Err(CompactError::UnsupportedSchema(format!(
                "{path}: keyword '{key}' is not supported for compaction"
            )));
        }
    }
    Ok(())
}

/// Rebuild a JSON Schema object from [`ParamSpec`]s (for `decode_tools` round-trip).
pub(crate) fn params_to_schema(params: &[ParamSpec]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in params {
        if p.required {
            required.push(Value::String(p.name.clone()));
        }
        properties.insert(
            p.name.clone(),
            type_expr_to_schema(&p.type_expr, p.description.as_deref()),
        );
    }
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    // Fail-closed default for reconstructed schemas.
    schema.insert("additionalProperties".into(), Value::Bool(false));
    Value::Object(schema)
}

fn type_expr_to_schema(expr: &TypeExpr, description: Option<&str>) -> Value {
    let mut m = Map::new();
    match expr {
        TypeExpr::Str => {
            m.insert("type".into(), Value::String("string".into()));
        }
        TypeExpr::Datetime => {
            m.insert("type".into(), Value::String("string".into()));
            m.insert("format".into(), Value::String("date-time".into()));
        }
        TypeExpr::Date => {
            m.insert("type".into(), Value::String("string".into()));
            m.insert("format".into(), Value::String("date".into()));
        }
        TypeExpr::Time => {
            m.insert("type".into(), Value::String("string".into()));
            m.insert("format".into(), Value::String("time".into()));
        }
        TypeExpr::Uri => {
            m.insert("type".into(), Value::String("string".into()));
            m.insert("format".into(), Value::String("uri".into()));
        }
        TypeExpr::Int => {
            m.insert("type".into(), Value::String("integer".into()));
        }
        TypeExpr::Num => {
            m.insert("type".into(), Value::String("number".into()));
        }
        TypeExpr::Bool => {
            m.insert("type".into(), Value::String("boolean".into()));
        }
        TypeExpr::Null => {
            m.insert("type".into(), Value::String("null".into()));
        }
        TypeExpr::Enum(variants) => {
            m.insert("type".into(), Value::String("string".into()));
            m.insert(
                "enum".into(),
                Value::Array(variants.iter().cloned().map(Value::String).collect()),
            );
        }
        TypeExpr::Array(inner) => {
            m.insert("type".into(), Value::String("array".into()));
            m.insert("items".into(), type_expr_to_schema(inner, None));
        }
        TypeExpr::Object(fields) => {
            return params_to_schema(fields);
        }
        TypeExpr::OpaqueObject => {
            m.insert("type".into(), Value::String("object".into()));
        }
    }
    if let Some(d) = description {
        m.insert("description".into(), Value::String(d.to_string()));
    }
    Value::Object(m)
}

/// Whether the schema allows additional properties (default: **false** — fail-closed).
pub(crate) fn allows_additional(parameters: Option<&Value>) -> bool {
    parameters
        .and_then(|p| p.get("additionalProperties"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
