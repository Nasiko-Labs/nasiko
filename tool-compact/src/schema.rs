use std::collections::BTreeMap;

use jsonschema::Validator;
use serde_json::Value;

use crate::{CompactError, Result, ToolCall, ToolDef};

pub(crate) const MAX_SCHEMA_BYTES: usize = 1_048_576;
pub(crate) const MAX_SCHEMA_DEPTH: usize = 32;
pub(crate) const MAX_TOOLS: usize = 256;

/// Validators are compiled once per catalog, not for every output chunk/call.
pub(crate) struct Catalog {
    validators: BTreeMap<String, Option<Validator>>,
}

impl Catalog {
    pub(crate) fn new(tools: &[ToolDef]) -> Result<Self> {
        if tools.len() > MAX_TOOLS {
            return Err(CompactError::LimitExceeded);
        }
        let mut validators = BTreeMap::new();
        let mut bytes = 0usize;
        for tool in tools {
            if tool
                .name
                .len()
                .saturating_add(tool.description.as_ref().map_or(0, String::len))
                > MAX_SCHEMA_BYTES
            {
                return Err(CompactError::LimitExceeded);
            }
            if let Some(parameters) = &tool.parameters {
                check_value_depth(parameters, 0)?;
            }
            bytes = bytes.saturating_add(
                serde_json::to_vec(tool)
                    .map_err(|_| CompactError::InvalidSchema("tool is not serializable".into()))?
                    .len(),
            );
            if bytes > MAX_SCHEMA_BYTES {
                return Err(CompactError::LimitExceeded);
            }
            if !is_name(&tool.name) || tool.name.len() > 64 {
                return Err(CompactError::InvalidSchema("invalid tool name".into()));
            }
            let validator = tool.parameters.as_ref().map(compile).transpose()?;
            if validators.insert(tool.name.clone(), validator).is_some() {
                return Err(CompactError::InvalidSchema("duplicate tool name".into()));
            }
        }
        Ok(Self { validators })
    }

    pub(crate) fn validate(&self, call: &ToolCall) -> Result<()> {
        let validator = self
            .validators
            .get(&call.name)
            .ok_or(CompactError::UnknownTool)?;
        if !call.arguments.is_object() {
            return Err(CompactError::InvalidArguments);
        }
        if let Some(validator) = validator {
            if !validator.is_valid(&call.arguments) {
                return Err(CompactError::InvalidArguments);
            }
        } else if call
            .arguments
            .as_object()
            .is_some_and(|args| !args.is_empty())
        {
            // An omitted parameters definition denotes a function without parameters.
            return Err(CompactError::InvalidArguments);
        }
        Ok(())
    }
}

// Metadata such as defaults/enums may contain arbitrary JSON, not just child schemas.
// Bound it before serialization or recursive schema compilation too.
fn check_value_depth(value: &Value, depth: usize) -> Result<()> {
    if depth > 96 {
        return Err(CompactError::LimitExceeded);
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_SCHEMA_BYTES {
                return Err(CompactError::LimitExceeded);
            }
            for value in values {
                check_value_depth(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            if values.len() > MAX_SCHEMA_BYTES {
                return Err(CompactError::LimitExceeded);
            }
            for (key, value) in values {
                if key.len() > MAX_SCHEMA_BYTES {
                    return Err(CompactError::LimitExceeded);
                }
                check_value_depth(value, depth + 1)?;
            }
        }
        Value::String(value) if value.len() > MAX_SCHEMA_BYTES => {
            return Err(CompactError::LimitExceeded);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn compile(schema: &Value) -> Result<Validator> {
    check_supported(schema, 0)?;
    if !jsonschema::draft202012::meta::is_valid(schema) {
        return Err(CompactError::InvalidSchema("invalid JSON Schema".into()));
    }
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .build(schema)
        .map_err(|_| CompactError::InvalidSchema("schema validation could not be compiled".into()))
}

/// Explicit allowlist prevents a new/unknown keyword from silently losing its meaning.
/// References are never resolved, even if another workspace crate enables resolver features.
fn check_supported(schema: &Value, depth: usize) -> Result<()> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(CompactError::LimitExceeded);
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| CompactError::InvalidSchema("schema must be an object or boolean".into()))?;
    for (key, value) in object {
        match key.as_str() {
            "type" | "enum" | "const" | "required" | "title" | "description" | "default"
            | "examples" | "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum"
            | "multipleOf" | "minLength" | "maxLength" | "minItems" | "maxItems"
            | "uniqueItems" | "minProperties" | "maxProperties" => {}
            "format" => {
                if !matches!(
                    value.as_str(),
                    Some(
                        "date-time"
                            | "date"
                            | "time"
                            | "email"
                            | "hostname"
                            | "ipv4"
                            | "ipv6"
                            | "uuid"
                            | "uri"
                            | "uri-reference"
                    )
                ) {
                    return Err(CompactError::UnsupportedSchema("format".into()));
                }
            }
            "properties" => {
                let properties = value.as_object().ok_or_else(|| {
                    CompactError::InvalidSchema("properties must be an object".into())
                })?;
                for child in properties.values() {
                    check_supported(child, depth + 1)?;
                }
            }
            "items" | "additionalProperties" => check_supported(value, depth + 1)?,
            // Patterns and compositions need independent complexity controls; bypass for now.
            _ => return Err(CompactError::UnsupportedSchema(key.clone())),
        }
    }
    Ok(())
}
