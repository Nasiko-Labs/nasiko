//! Schema intermediate representation between OpenAI JSON Schema and compact text.
//!
//! The IR captures the subset of JSON Schema this crate can represent and validate.
//! Tools whose schemas use unsupported features are **bypassed** (kept native) rather
//! than silently dropping constraints.
//!
//! # Supported schema subset
//!
//! - Primitive types: `string`, `integer`, `number`, `boolean`
//! - String formats: `date-time`, `date` (other formats → plain string)
//! - String enums (all values must be strings without `|`, whitespace, or quotes)
//! - Arrays with typed items
//! - Nested objects with required/optional fields
//! - `additionalProperties: false` or `true` (boolean only)
//!
//! # Unsupported features (trigger tool bypass)
//!
//! Composition: `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, `if`/`then`/`else`
//! Complex additional properties: `patternProperties`, `additionalProperties: {schema}`
//! Format-less constraints: `minimum`, `maximum`, `exclusiveMinimum`,
//! `exclusiveMaximum`, `minLength`, `maxLength`, `pattern`, `default`, `const`,
//! `multipleOf`, `uniqueItems`, `minItems`, `maxItems`, `minProperties`,
//! `maxProperties`

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::CompactError;

/// Keywords whose presence in a JSON Schema object triggers bypass.
const UNSUPPORTED_KEYWORDS: &[&str] = &[
    // Composition
    "$ref",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    // Complex additional properties
    "patternProperties",
    // Format-less constraints (cannot represent or validate)
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "pattern",
    "default",
    "const",
    "multipleOf",
    "uniqueItems",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
];

// ── IR types ───────────────────────────────────────────────────────────────

/// A schema type in the IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaType {
    String,
    Integer,
    Number,
    Boolean,
    DateTime,
    Date,
    /// All enum values are plain strings.
    StringEnum(Vec<String>),
    Array(Box<SchemaType>),
    Object(BTreeMap<String, FieldIR>),
}

/// One field inside an object schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIR {
    pub schema: SchemaType,
    pub required: bool,
    pub description: Option<String>,
}

/// A complete tool parsed into the IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIR {
    pub name: String,
    pub params: BTreeMap<String, FieldIR>,
    pub description: Option<String>,
}

// ── Parsing (JSON Schema → IR) ─────────────────────────────────────────────

impl ToolIR {
    /// Parse an OpenAI tool schema into the IR.
    ///
    /// Returns [`CompactError::UnsupportedSchema`] if the schema uses features
    /// outside the supported subset.
    pub fn from_json_schema(
        name: &str,
        description: Option<&str>,
        schema: &Value,
    ) -> Result<Self, CompactError> {
        check_unsupported(name, schema)?;

        let params = parse_object_fields(name, schema)?;

        Ok(Self {
            name: name.to_string(),
            params,
            description: description.map(String::from),
        })
    }

    /// Round-trip back to a JSON Schema `Value`.
    pub fn to_json_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (key, field) in &self.params {
            let mut prop = type_to_json(&field.schema);
            if let Some(desc) = &field.description
                && let Some(obj) = prop.as_object_mut()
            {
                obj.insert("description".to_string(), Value::String(desc.clone()));
            }
            properties.insert(key.clone(), prop);
            if field.required {
                required.push(Value::String(key.clone()));
            }
        }

        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), Value::String("object".to_string()));
        if !properties.is_empty() {
            schema.insert("properties".to_string(), Value::Object(properties));
        }
        if !required.is_empty() {
            schema.insert("required".to_string(), Value::Array(required));
        }

        Value::Object(schema)
    }
}

fn parse_object_fields(
    tool: &str,
    schema: &Value,
) -> Result<BTreeMap<String, FieldIR>, CompactError> {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(BTreeMap::new());
    };

    let required_fields: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let mut result = BTreeMap::new();
    for (key, val) in props {
        check_unsupported(tool, val)?;
        let schema_type = parse_type(tool, val)?;
        let desc = val
            .get("description")
            .and_then(Value::as_str)
            .map(String::from);
        result.insert(
            key.clone(),
            FieldIR {
                schema: schema_type,
                required: required_fields.contains(key),
                description: desc,
            },
        );
    }
    Ok(result)
}

/// Check for unsupported JSON Schema keywords. `additionalProperties` is allowed
/// when its value is a boolean.
fn check_unsupported(tool: &str, schema: &Value) -> Result<(), CompactError> {
    let Some(obj) = schema.as_object() else {
        return Ok(());
    };
    for keyword in UNSUPPORTED_KEYWORDS {
        if obj.contains_key(*keyword) {
            return Err(CompactError::UnsupportedSchema {
                tool: tool.to_string(),
                feature: (*keyword).to_string(),
            });
        }
    }
    // additionalProperties: handled separately — booleans are fine, objects are not.
    if let Some(ap) = obj.get("additionalProperties")
        && !ap.is_boolean()
    {
        return Err(CompactError::UnsupportedSchema {
            tool: tool.to_string(),
            feature: "additionalProperties (non-boolean)".to_string(),
        });
    }
    Ok(())
}

fn parse_type(tool: &str, schema: &Value) -> Result<SchemaType, CompactError> {
    // Enum takes priority over type.
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        let mut values = Vec::new();
        for val in enum_values {
            match val {
                Value::String(s) => {
                    if s.contains('|')
                        || s.contains('"')
                        || s.contains('\'')
                        || s.chars().any(|c| c.is_whitespace())
                    {
                        return Err(CompactError::UnsupportedSchema {
                            tool: tool.to_string(),
                            feature: format!("enum value with special characters: {s:?}"),
                        });
                    }
                    values.push(s.clone());
                }
                _ => {
                    return Err(CompactError::UnsupportedSchema {
                        tool: tool.to_string(),
                        feature: "non-string enum value".to_string(),
                    });
                }
            }
        }
        return Ok(SchemaType::StringEnum(values));
    }

    let type_str = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string");

    match type_str {
        "string" => {
            if let Some(format) = schema.get("format").and_then(Value::as_str) {
                match format {
                    "date-time" => Ok(SchemaType::DateTime),
                    "date" => Ok(SchemaType::Date),
                    _ => Ok(SchemaType::String),
                }
            } else {
                Ok(SchemaType::String)
            }
        }
        "integer" => Ok(SchemaType::Integer),
        "number" => Ok(SchemaType::Number),
        "boolean" => Ok(SchemaType::Boolean),
        "array" => {
            if let Some(items) = schema.get("items") {
                check_unsupported(tool, items)?;
                let item_type = parse_type(tool, items)?;
                Ok(SchemaType::Array(Box::new(item_type)))
            } else {
                // No items schema — default to string elements.
                Ok(SchemaType::Array(Box::new(SchemaType::String)))
            }
        }
        "object" => {
            let fields = parse_object_fields(tool, schema)?;
            Ok(SchemaType::Object(fields))
        }
        other => Err(CompactError::UnsupportedSchema {
            tool: tool.to_string(),
            feature: format!("unknown type: {other}"),
        }),
    }
}

// ── Rendering (IR → JSON Schema) ──────────────────────────────────────────

fn type_to_json(schema: &SchemaType) -> Value {
    match schema {
        SchemaType::String => serde_json::json!({"type": "string"}),
        SchemaType::Integer => serde_json::json!({"type": "integer"}),
        SchemaType::Number => serde_json::json!({"type": "number"}),
        SchemaType::Boolean => serde_json::json!({"type": "boolean"}),
        SchemaType::DateTime => serde_json::json!({"type": "string", "format": "date-time"}),
        SchemaType::Date => serde_json::json!({"type": "string", "format": "date"}),
        SchemaType::StringEnum(values) => {
            serde_json::json!({"type": "string", "enum": values})
        }
        SchemaType::Array(item) => {
            serde_json::json!({"type": "array", "items": type_to_json(item)})
        }
        SchemaType::Object(fields) => {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for (key, field) in fields {
                let mut prop = type_to_json(&field.schema);
                if let Some(desc) = &field.description
                    && let Some(obj) = prop.as_object_mut()
                {
                    obj.insert("description".to_string(), Value::String(desc.clone()));
                }
                properties.insert(key.clone(), prop);
                if field.required {
                    required.push(Value::String(key.clone()));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), Value::String("object".to_string()));
            if !properties.is_empty() {
                obj.insert("properties".to_string(), Value::Object(properties));
            }
            if !required.is_empty() {
                obj.insert("required".to_string(), Value::Array(required));
            }
            Value::Object(obj)
        }
    }
}

// ── Compact text rendering ─────────────────────────────────────────────────

/// Render one type in the compact notation.
pub(crate) fn render_type(schema: &SchemaType) -> String {
    match schema {
        SchemaType::String => "str".to_string(),
        SchemaType::Integer => "int".to_string(),
        SchemaType::Number => "num".to_string(),
        SchemaType::Boolean => "bool".to_string(),
        SchemaType::DateTime => "datetime".to_string(),
        SchemaType::Date => "date".to_string(),
        SchemaType::StringEnum(values) => values.join("|"),
        SchemaType::Array(item) => format!("[{}]", render_type(item)),
        SchemaType::Object(fields) => {
            let params: Vec<String> = fields
                .iter()
                .map(|(name, field)| {
                    let opt = if field.required { "" } else { "?" };
                    format!("{name}{opt}:{}", render_type(&field.schema))
                })
                .collect();
            format!("{{{}}}", params.join(", "))
        }
    }
}

/// Build a realistic example `Value` for a schema type (for the example call).
pub(crate) fn example_value(schema: &SchemaType) -> Value {
    match schema {
        SchemaType::String => Value::String("example".to_string()),
        SchemaType::Integer => Value::Number(42.into()),
        SchemaType::Number => serde_json::Number::from_f64(3.5)
            .map(Value::Number)
            .unwrap_or(Value::Number(0.into())),
        SchemaType::Boolean => Value::Bool(true),
        SchemaType::DateTime => Value::String("2026-10-05T15:00:00+05:30".to_string()),
        SchemaType::Date => Value::String("2026-10-05".to_string()),
        SchemaType::StringEnum(values) => Value::String(
            values
                .first()
                .cloned()
                .unwrap_or_else(|| "value".to_string()),
        ),
        SchemaType::Array(item) => Value::Array(vec![example_value(item)]),
        SchemaType::Object(fields) => {
            let mut obj = serde_json::Map::new();
            for (key, field) in fields {
                if field.required {
                    obj.insert(key.clone(), example_value(&field.schema));
                }
            }
            Value::Object(obj)
        }
    }
}

/// Public list of unsupported keywords for docs / crate-level documentation.
pub const UNSUPPORTED_KEYWORD_LIST: &[&str] = UNSUPPORTED_KEYWORDS;
