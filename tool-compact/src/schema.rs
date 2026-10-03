use serde_json::{Map, Value};

use crate::CompactError;

const STRUCTURAL_MAP_KEYS: &[&str] = &[
    "properties",
    "patternProperties",
    "dependentSchemas",
    "$defs",
    "definitions",
];
const STRUCTURAL_SINGLE_KEYS: &[&str] = &[
    "items",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "contains",
    "not",
    "if",
    "then",
    "else",
    "propertyNames",
    "contentSchema",
];
const STRUCTURAL_ARRAY_KEYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

pub(crate) fn validate_supported(schema: &Value) -> Result<(), CompactError> {
    reject_references(schema, "$")?;
    jsonschema::options()
        .offline()
        .should_validate_formats(true)
        .build(schema)
        .map(|_| ())
        .map_err(|error| CompactError::UnsupportedSchema {
            path: "$".to_string(),
            feature: format!("invalid JSON Schema: {error}"),
        })
}

pub(crate) fn compact_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(object) => {
            let mut compact = Map::new();
            for (key, value) in object {
                let compact_key = alias(key).map_or_else(|| format!("~{key}"), str::to_string);
                let compact_value = compact_keyword_value(key, value);
                compact.insert(compact_key, compact_value);
            }
            Value::Object(compact)
        }
        other => other.clone(),
    }
}

pub(crate) fn expand_schema(schema: &Value) -> Result<Value, CompactError> {
    match schema {
        Value::Object(object) => {
            let mut expanded = Map::new();
            for (key, value) in object {
                let original_key = unalias(key)?;
                let expanded_value = expand_keyword_value(&original_key, value)?;
                if expanded.insert(original_key.clone(), expanded_value).is_some() {
                    return Err(CompactError::InvalidCompactEncoding(format!(
                        "schema key '{original_key}' occurs more than once"
                    )));
                }
            }
            Ok(Value::Object(expanded))
        }
        other => Ok(other.clone()),
    }
}

fn compact_keyword_value(key: &str, value: &Value) -> Value {
    if key == "type" {
        return compact_type(value);
    }
    if STRUCTURAL_MAP_KEYS.contains(&key) {
        return map_schema_values(value, compact_schema);
    }
    if STRUCTURAL_SINGLE_KEYS.contains(&key) {
        return match value {
            Value::Object(_) | Value::Bool(_) => compact_schema(value),
            _ => value.clone(),
        };
    }
    if STRUCTURAL_ARRAY_KEYS.contains(&key) {
        return array_schema_values(value, compact_schema);
    }
    value.clone()
}

fn expand_keyword_value(key: &str, value: &Value) -> Result<Value, CompactError> {
    if key == "type" {
        return Ok(expand_type(value));
    }
    if STRUCTURAL_MAP_KEYS.contains(&key) {
        return try_map_schema_values(value);
    }
    if STRUCTURAL_SINGLE_KEYS.contains(&key) {
        return match value {
            Value::Object(_) | Value::Bool(_) => expand_schema(value),
            _ => Ok(value.clone()),
        };
    }
    if STRUCTURAL_ARRAY_KEYS.contains(&key) {
        return try_array_schema_values(value);
    }
    Ok(value.clone())
}

fn map_schema_values(value: &Value, transform: fn(&Value) -> Value) -> Value {
    let Value::Object(object) = value else {
        return value.clone();
    };
    Value::Object(
        object
            .iter()
            .map(|(key, schema)| (key.clone(), transform(schema)))
            .collect(),
    )
}

fn array_schema_values(value: &Value, transform: fn(&Value) -> Value) -> Value {
    let Value::Array(items) = value else {
        return value.clone();
    };
    Value::Array(items.iter().map(transform).collect())
}

fn try_map_schema_values(value: &Value) -> Result<Value, CompactError> {
    let Value::Object(object) = value else {
        return Ok(value.clone());
    };
    let expanded = object
        .iter()
        .map(|(key, schema)| Ok((key.clone(), expand_schema(schema)?)))
        .collect::<Result<Map<_, _>, CompactError>>()?;
    Ok(Value::Object(expanded))
}

fn try_array_schema_values(value: &Value) -> Result<Value, CompactError> {
    let Value::Array(items) = value else {
        return Ok(value.clone());
    };
    items
        .iter()
        .map(expand_schema)
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn compact_type(value: &Value) -> Value {
    match value {
        Value::String(kind) => Value::String(type_alias(kind).to_string()),
        Value::Array(kinds) => Value::Array(kinds.iter().map(compact_type).collect()),
        _ => value.clone(),
    }
}

fn expand_type(value: &Value) -> Value {
    match value {
        Value::String(kind) => Value::String(type_unalias(kind).to_string()),
        Value::Array(kinds) => Value::Array(kinds.iter().map(expand_type).collect()),
        _ => value.clone(),
    }
}

fn reject_references(value: &Value, path: &str) -> Result<(), CompactError> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}/{key}");
                if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                    return Err(CompactError::UnsupportedSchema {
                        path: child_path,
                        feature: key.clone(),
                    });
                }
                reject_references(child, &child_path)?;
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                reject_references(child, &format!("{path}/{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn alias(key: &str) -> Option<&'static str> {
    Some(match key {
        "type" => "t",
        "properties" => "p",
        "required" => "r",
        "items" => "i",
        "enum" => "e",
        "description" => "d",
        "format" => "f",
        "additionalProperties" => "a",
        "$schema" => "$",
        "title" => "l",
        "default" => "v",
        "examples" => "x",
        "const" => "c",
        "minimum" => "mn",
        "maximum" => "mx",
        "exclusiveMinimum" => "en",
        "exclusiveMaximum" => "ex",
        "multipleOf" => "mu",
        "minLength" => "nl",
        "maxLength" => "xl",
        "pattern" => "pt",
        "minItems" => "ni",
        "maxItems" => "xi",
        "uniqueItems" => "u",
        "minProperties" => "np",
        "maxProperties" => "xp",
        "allOf" => "&",
        "anyOf" => "|",
        "oneOf" => "1",
        "not" => "!",
        "if" => "?",
        "then" => "+",
        "else" => "-",
        "prefixItems" => "pi",
        "contains" => "co",
        "minContains" => "nc",
        "maxContains" => "xc",
        "patternProperties" => "pp",
        "propertyNames" => "pn",
        "dependentRequired" => "dr",
        "dependentSchemas" => "ds",
        "unevaluatedProperties" => "up",
        "unevaluatedItems" => "ui",
        "$defs" => "df",
        "definitions" => "de",
        "readOnly" => "ro",
        "writeOnly" => "wo",
        "deprecated" => "dp",
        "contentEncoding" => "ce",
        "contentMediaType" => "cm",
        "contentSchema" => "cs",
        _ => return None,
    })
}

fn unalias(key: &str) -> Result<String, CompactError> {
    let original = match key {
        "t" => "type",
        "p" => "properties",
        "r" => "required",
        "i" => "items",
        "e" => "enum",
        "d" => "description",
        "f" => "format",
        "a" => "additionalProperties",
        "$" => "$schema",
        "l" => "title",
        "v" => "default",
        "x" => "examples",
        "c" => "const",
        "mn" => "minimum",
        "mx" => "maximum",
        "en" => "exclusiveMinimum",
        "ex" => "exclusiveMaximum",
        "mu" => "multipleOf",
        "nl" => "minLength",
        "xl" => "maxLength",
        "pt" => "pattern",
        "ni" => "minItems",
        "xi" => "maxItems",
        "u" => "uniqueItems",
        "np" => "minProperties",
        "xp" => "maxProperties",
        "&" => "allOf",
        "|" => "anyOf",
        "1" => "oneOf",
        "!" => "not",
        "?" => "if",
        "+" => "then",
        "-" => "else",
        "pi" => "prefixItems",
        "co" => "contains",
        "nc" => "minContains",
        "xc" => "maxContains",
        "pp" => "patternProperties",
        "pn" => "propertyNames",
        "dr" => "dependentRequired",
        "ds" => "dependentSchemas",
        "up" => "unevaluatedProperties",
        "ui" => "unevaluatedItems",
        "df" => "$defs",
        "de" => "definitions",
        "ro" => "readOnly",
        "wo" => "writeOnly",
        "dp" => "deprecated",
        "ce" => "contentEncoding",
        "cm" => "contentMediaType",
        "cs" => "contentSchema",
        unknown if unknown.starts_with('~') => return Ok(unknown[1..].to_string()),
        unknown => {
            return Err(CompactError::InvalidCompactEncoding(format!(
                "unknown schema key alias '{unknown}'"
            )));
        }
    };
    Ok(original.to_string())
}

fn type_alias(kind: &str) -> &str {
    match kind {
        "object" => "o",
        "array" => "a",
        "string" => "s",
        "integer" => "i",
        "number" => "n",
        "boolean" => "b",
        "null" => "0",
        other => other,
    }
}

fn type_unalias(kind: &str) -> &str {
    match kind {
        "o" => "object",
        "a" => "array",
        "s" => "string",
        "i" => "integer",
        "n" => "number",
        "b" => "boolean",
        "0" => "null",
        other => other,
    }
}

