mod schema;

pub use schema::{Schema, SchemaType, ToolCall, ToolDef};

pub mod encode;
pub mod decode;
pub mod validate;
pub mod stream;

use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("invalid tool schema: {0}")]
    InvalidSchema(String),

    #[error("unknown tool: {0}")]
    UnknownTool(String),

    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),

    #[error("malformed compact call: {0}")]
    MalformedCall(String),

    #[error("unsupported schema feature: {0}")]
    UnsupportedSchema(String),
}

pub fn parse_schema(value: &Value) -> Result<Schema, CompactError> {
    let schema_type = match value.get("type").and_then(Value::as_str) {
        Some("string") => SchemaType::String,
        Some("integer") => SchemaType::Integer,
        Some("number") => SchemaType::Number,
        Some("boolean") => SchemaType::Boolean,
        Some("object") => SchemaType::Object,
        Some("array") => SchemaType::Array,
        Some(other) => SchemaType::Unknown(other.to_string()),
        None => {
            return Err(CompactError::InvalidSchema(
                "missing schema type".into(),
            ))
        }
    };

    let mut schema = Schema::new(schema_type);

    // Preserve JSON Schema format such as "date-time".
    if let Some(format) = value.get("format").and_then(Value::as_str) {
        schema.format = Some(format.to_string());
    }

    if let Some(required) = value.get("required").and_then(Value::as_array) {
        for item in required {
            if let Some(name) = item.as_str() {
                schema.required.push(name.to_string());
            }
        }
    }

    if let Some(properties) = value.get("properties").and_then(Value::as_object) {
        for (name, property) in properties {
            schema
                .properties
                .push((name.clone(), parse_schema(property)?));
        }
    }

    if let Some(items) = value.get("items") {
        schema.items = Some(Box::new(parse_schema(items)?));
    }

    if let Some(values) = value.get("enum").and_then(Value::as_array) {
        for value in values {
            if let Some(value) = value.as_str() {
                schema.enum_values.push(value.to_string());
            } else {
                return Err(CompactError::UnsupportedSchema(
                    "only string enums are currently supported".into(),
                ));
            }
        }
    }

    if let SchemaType::Unknown(ref t) = schema.schema_type {
        return Err(CompactError::UnsupportedSchema(t.clone()));
    }

    Ok(schema)
}