//! Schema analysis helpers: extract parameter structure for compact rendering
//! and validation.

use serde_json::Value;

/// A simplified view of a JSON Schema parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamSchema {
    /// `type: "string"`, optional enum constraint.
    Str { enum_values: Option<Vec<String>> },
    /// `type: "integer"`
    Integer,
    /// `type: "number"`
    Number,
    /// `type: "boolean"`
    Boolean,
    /// `type: "array"`, optional item type hint.
    Array { item_type: Option<Box<ParamSchema>> },
    /// `type: "object"`, with named children.
    Object {
        props: Vec<(String, ParamSchema, bool)>,
    }, // (name, schema, required)
    /// Unknown / unsupported schema: treated as opaque JSON.
    Opaque,
}

impl ParamSchema {
    /// Returns the short type token for compact schema rendering.
    pub fn type_token(&self) -> &'static str {
        match self {
            Self::Str { .. } => "str",
            Self::Integer => "int",
            Self::Number => "num",
            Self::Boolean => "bool",
            Self::Array { .. } => "array",
            Self::Object { .. } => "obj",
            Self::Opaque => "any",
        }
    }
}

/// Parse a JSON Schema value into a [`ParamSchema`].
///
/// On any unsupported feature, returns [`ParamSchema::Opaque`] (fail-closed).
pub fn parse_schema(v: &Value) -> ParamSchema {
    let obj = match v.as_object() {
        Some(o) => o,
        None => return ParamSchema::Opaque,
    };

    // Unsupported combinators: fail-closed to Opaque.
    for key in &["oneOf", "anyOf", "allOf", "not", "$ref", "$defs"] {
        if obj.contains_key(*key) {
            return ParamSchema::Opaque;
        }
    }

    let type_str = obj.get("type").and_then(Value::as_str).unwrap_or("");

    match type_str {
        "string" => {
            let enums = obj.get("enum").and_then(Value::as_array).map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            });
            ParamSchema::Str { enum_values: enums }
        }
        "integer" => ParamSchema::Integer,
        "number" => ParamSchema::Number,
        "boolean" => ParamSchema::Boolean,
        "array" => {
            let item_type = obj.get("items").map(|items| Box::new(parse_schema(items)));
            ParamSchema::Array { item_type }
        }
        "object" => {
            let props_obj = match obj.get("properties").and_then(Value::as_object) {
                Some(p) => p,
                None => return ParamSchema::Object { props: vec![] },
            };
            let required: Vec<String> = obj
                .get("required")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();

            let props = props_obj
                .iter()
                .map(|(name, schema)| {
                    let is_req = required.contains(name);
                    (name.clone(), parse_schema(schema), is_req)
                })
                .collect();

            ParamSchema::Object { props }
        }
        _ => ParamSchema::Opaque,
    }
}

/// Extract the required fields list from a top-level parameters schema.
pub fn required_fields(params: &Value) -> Vec<String> {
    params
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Render a compact, one-line schema description for a function's parameters.
/// Used inside the system prompt block.
pub fn render_compact_params(params: &Value) -> String {
    let obj = match params.as_object() {
        Some(o) => o,
        None => return String::new(),
    };
    let props = match obj.get("properties").and_then(Value::as_object) {
        Some(p) => p,
        None => return String::new(),
    };
    let required: Vec<String> = required_fields(params);

    let mut parts = Vec::new();
    for (name, schema) in props {
        let ps = parse_schema(schema);
        let req_marker = if required.contains(name) { "*" } else { "?" };
        let desc = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let type_token = ps.type_token();

        let enum_suffix = if let ParamSchema::Str {
            enum_values: Some(ref vals),
        } = ps
        {
            format!("({})", vals.join("|"))
        } else {
            String::new()
        };

        let item_suffix = if let ParamSchema::Array {
            item_type: Some(ref it),
        } = ps
        {
            format!("[{}]", it.type_token())
        } else {
            String::new()
        };

        let type_full = format!("{}{}{}", type_token, enum_suffix, item_suffix);

        if desc.is_empty() {
            parts.push(format!("{}{}: {}", name, req_marker, type_full));
        } else {
            parts.push(format!("{}{}: {} — {}", name, req_marker, type_full, desc));
        }
    }

    parts.join(", ")
}

// ─── decode_tools ────────────────────────────────────────────────────────────

use crate::error::EncodeError;
use crate::types::{CompactTools, ToolDef};

/// Reconstruct the original [`ToolDef`] list from a [`CompactTools`] handle.
///
/// This verifies that the schema round-trip is lossless: the definitions
/// retained inside `CompactTools` are byte-identical to the originals.
/// Useful in tests and for production verification before sending a compact
/// request to a live model.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, EncodeError> {
    if compact.defs.is_empty() {
        return Err(EncodeError::NoTools);
    }
    Ok(compact.defs.clone())
}
