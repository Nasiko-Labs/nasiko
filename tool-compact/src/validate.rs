//! Schema-aware argument validation (fail-closed).

use serde_json::Value;

use crate::error::{CompactError, Result};
use crate::schema::{allows_additional, parse_parameters};
use crate::types::{ToolDef, TypeExpr};

/// Validate `arguments` against `tool.parameters`.
pub fn validate_arguments(tool: &ToolDef, arguments: &Value) -> Result<()> {
    let obj = arguments
        .as_object()
        .ok_or_else(|| CompactError::InvalidArguments("arguments must be a JSON object".into()))?;

    let params = parse_parameters(tool.parameters.as_ref())?;
    let allow_extra = allows_additional(tool.parameters.as_ref());

    for p in &params {
        match obj.get(&p.name) {
            None if p.required => {
                return Err(CompactError::MissingRequiredField(p.name.clone()));
            }
            None => {}
            Some(v) => validate_value(&p.type_expr, v, &p.name)?,
        }
    }

    if !allow_extra {
        for key in obj.keys() {
            if !params.iter().any(|p| p.name == *key) {
                return Err(CompactError::InvalidArguments(format!(
                    "unknown field '{key}'"
                )));
            }
        }
    }

    Ok(())
}

fn validate_value(expr: &TypeExpr, value: &Value, path: &str) -> Result<()> {
    match expr {
        TypeExpr::Str | TypeExpr::Datetime | TypeExpr::Date | TypeExpr::Time | TypeExpr::Uri => {
            if !value.is_string() {
                return Err(CompactError::InvalidArguments(format!(
                    "{path}: expected string"
                )));
            }
            Ok(())
        }
        TypeExpr::Int => {
            // JSON numbers: accept integers only (reject 1.5).
            match value {
                Value::Number(n) if n.is_i64() || n.is_u64() => Ok(()),
                Value::Number(n) if n.as_f64().is_some_and(|f| f.fract() == 0.0) => Ok(()),
                _ => Err(CompactError::InvalidArguments(format!(
                    "{path}: expected integer"
                ))),
            }
        }
        TypeExpr::Num => {
            if value.is_number() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments(format!(
                    "{path}: expected number"
                )))
            }
        }
        TypeExpr::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments(format!(
                    "{path}: expected boolean"
                )))
            }
        }
        TypeExpr::Null => {
            if value.is_null() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments(format!(
                    "{path}: expected null"
                )))
            }
        }
        TypeExpr::Enum(variants) => {
            let s = value.as_str().ok_or_else(|| {
                CompactError::InvalidArguments(format!("{path}: expected string enum"))
            })?;
            if variants.iter().any(|v| v == s) {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments(format!(
                    "{path}: invalid enum value {s:?}"
                )))
            }
        }
        TypeExpr::Array(inner) => {
            let arr = value
                .as_array()
                .ok_or_else(|| CompactError::InvalidArguments(format!("{path}: expected array")))?;
            for (i, item) in arr.iter().enumerate() {
                validate_value(inner, item, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        TypeExpr::OpaqueObject => {
            if value.is_object() {
                Ok(())
            } else {
                Err(CompactError::InvalidArguments(format!(
                    "{path}: expected object"
                )))
            }
        }
        TypeExpr::Object(fields) => {
            let obj = value.as_object().ok_or_else(|| {
                CompactError::InvalidArguments(format!("{path}: expected object"))
            })?;
            for f in fields {
                match obj.get(&f.name) {
                    None if f.required => {
                        return Err(CompactError::MissingRequiredField(format!(
                            "{path}.{}",
                            f.name
                        )));
                    }
                    None => {}
                    Some(v) => validate_value(&f.type_expr, v, &format!("{path}.{}", f.name))?,
                }
            }
            // Nested objects: fail-closed on extras.
            for key in obj.keys() {
                if !fields.iter().any(|f| f.name == *key) {
                    return Err(CompactError::InvalidArguments(format!(
                        "{path}: unknown field '{key}'"
                    )));
                }
            }
            Ok(())
        }
    }
}

/// Look up a tool by name (exact match).
pub fn find_tool<'a>(tools: &'a [ToolDef], name: &str) -> Result<&'a ToolDef> {
    tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| CompactError::UnknownTool(name.to_string()))
}
