//! JSON Schema parser and deterministic Compact DSL encoder.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::error::CompactToolError;
use crate::types::{CompactTools, PropertySchema, ToolDefinition, TypeSchema};

/// Encodes a list of tool definitions into a compact tools prompt block.
///
/// Returns `Err(CompactToolError::UnsupportedSchemaFeature)` if any tool definition
/// uses JSON Schema features that cannot be losslessly represented.
pub fn encode_tools(tools: &[ToolDefinition]) -> Result<CompactTools, CompactToolError> {
    if tools.is_empty() {
        return Ok(CompactTools::new("", 0));
    }

    let mut out = String::new();
    out.push_str("# Available Tools\n");
    out.push_str("You have access to the following tools:\n\n");

    for tool in tools {
        let compact_sig = encode_single_tool(tool)?;
        out.push_str(&compact_sig);
        out.push_str("\n\n");
    }

    out.push_str("To invoke a tool, output:\n");
    out.push_str("<<call tool_name {\"arg\": \"value\"}>>\n");
    out.push_str("You may output normal text before and after tool calls. You may invoke multiple tools in one turn.");

    Ok(CompactTools::new(out, tools.len()))
}

/// Encodes a single tool definition into its compact signature.
pub fn encode_single_tool(tool: &ToolDefinition) -> Result<String, CompactToolError> {
    let type_schema = if let Some(params) = &tool.parameters {
        parse_json_schema(&tool.name, params)?
    } else {
        TypeSchema::Object {
            properties: BTreeMap::new(),
            required: BTreeSet::new(),
            additional_properties: true,
        }
    };

    let mut out = String::new();
    out.push_str("tool ");
    out.push_str(&tool.name);

    match type_schema {
        TypeSchema::Object {
            properties,
            required,
            ..
        } => {
            if properties.is_empty() {
                out.push_str("()");
            } else {
                out.push_str("(\n");
                let mut first = true;
                for (prop_name, prop_schema) in properties {
                    if !first {
                        out.push_str(",\n");
                    }
                    first = false;

                    let is_required = required.contains(&prop_name);
                    out.push_str("  ");
                    out.push_str(&prop_name);
                    if !is_required {
                        out.push('?');
                    }
                    out.push_str(": ");
                    out.push_str(&format_type_schema(&prop_schema.schema));

                    if let Some(default) = &prop_schema.default {
                        out.push_str(" = ");
                        out.push_str(&default.to_string());
                    }

                    if let Some(desc) = &prop_schema.description {
                        let clean_desc = desc.replace('\n', " ").replace(']', "");
                        out.push_str(" [");
                        out.push_str(&clean_desc);
                        out.push(']');
                    }
                }
                out.push_str("\n)");
            }
        }
        other => {
            out.push_str("(input: ");
            out.push_str(&format_type_schema(&other));
            out.push(')');
        }
    }

    if let Some(desc) = &tool.description {
        out.push_str("\n  ");
        let clean_desc = desc.trim();
        out.push_str(clean_desc);
    }

    Ok(out)
}

/// Recursively parses a `serde_json::Value` JSON Schema into a `TypeSchema` AST.
///
/// Fail-closed: Rejects all unsupported JSON schema keywords with `UnsupportedSchemaFeature`.
pub fn parse_json_schema(tool_name: &str, schema: &Value) -> Result<TypeSchema, CompactToolError> {
    let Value::Object(map) = schema else {
        return Err(CompactToolError::UnsupportedSchemaFeature {
            tool: tool_name.to_string(),
            feature: "non-object schema root",
        });
    };

    // Check unsupported keywords
    let unsupported_keywords = [
        ("oneOf", "oneOf polymorphic union"),
        ("anyOf", "anyOf polymorphic union"),
        ("allOf", "allOf intersection schema"),
        ("$ref", "$ref reference schema"),
        ("$defs", "$defs schema definitions"),
        ("definitions", "definitions schema definitions"),
        (
            "patternProperties",
            "patternProperties regular expression schema",
        ),
        ("prefixItems", "prefixItems tuple schema"),
        ("if", "if/then/else conditional schema"),
        ("then", "if/then/else conditional schema"),
        ("else", "if/then/else conditional schema"),
        ("not", "not negated schema"),
        ("dependentRequired", "dependentRequired schema"),
        ("dependentSchemas", "dependentSchemas schema"),
        ("unevaluatedProperties", "unevaluatedProperties schema"),
        ("unevaluatedItems", "unevaluatedItems schema"),
    ];

    for (kw, feature) in unsupported_keywords {
        if map.contains_key(kw) {
            return Err(CompactToolError::UnsupportedSchemaFeature {
                tool: tool_name.to_string(),
                feature,
            });
        }
    }

    // Check schema-valued additionalProperties
    let additional_properties = if let Some(add_prop) = map.get("additionalProperties") {
        match add_prop {
            Value::Bool(b) => *b,
            Value::Object(_) => {
                return Err(CompactToolError::UnsupportedSchemaFeature {
                    tool: tool_name.to_string(),
                    feature: "schema-valued additionalProperties (dictionary schema)",
                });
            }
            _ => true,
        }
    } else {
        true
    };

    // Check string enums
    if let Some(enum_val) = map.get("enum") {
        if let Value::Array(variants) = enum_val {
            let mut string_variants = Vec::new();
            for v in variants {
                if let Value::String(s) = v {
                    string_variants.push(s.clone());
                } else {
                    return Err(CompactToolError::UnsupportedSchemaFeature {
                        tool: tool_name.to_string(),
                        feature: "non-string enum variant",
                    });
                }
            }
            return Ok(TypeSchema::Enum(string_variants));
        }
    }

    let type_str = map.get("type").and_then(Value::as_str);

    match type_str {
        Some("string") => Ok(TypeSchema::String),
        Some("integer") => Ok(TypeSchema::Integer),
        Some("number") => Ok(TypeSchema::Number),
        Some("boolean") => Ok(TypeSchema::Boolean),
        Some("null") => Ok(TypeSchema::Null),
        Some("array") => {
            let Some(items_val) = map.get("items") else {
                return Err(CompactToolError::UnsupportedSchemaFeature {
                    tool: tool_name.to_string(),
                    feature: "array schema missing 'items' property",
                });
            };
            let item_schema = parse_json_schema(tool_name, items_val)?;
            Ok(TypeSchema::Array(Box::new(item_schema)))
        }
        Some("object") | None => {
            let mut properties = BTreeMap::new();
            let mut required = BTreeSet::new();

            if let Some(Value::Array(req_arr)) = map.get("required") {
                for r in req_arr {
                    if let Value::String(field_name) = r {
                        required.insert(field_name.clone());
                    }
                }
            }

            if let Some(Value::Object(props_map)) = map.get("properties") {
                for (prop_name, prop_val) in props_map {
                    let prop_schema = parse_property_schema(tool_name, prop_val)?;
                    properties.insert(prop_name.clone(), prop_schema);
                }
            } else if type_str.is_none() && !map.contains_key("enum") && !map.contains_key("items")
            {
                // Empty or untyped schema with no properties
            }

            Ok(TypeSchema::Object {
                properties,
                required,
                additional_properties,
            })
        }
        Some(_other) => Err(CompactToolError::UnsupportedSchemaFeature {
            tool: tool_name.to_string(),
            feature: "unrecognized primitive type",
        }),
    }
}

/// Parses an individual property schema with its description and default value.
fn parse_property_schema(tool_name: &str, val: &Value) -> Result<PropertySchema, CompactToolError> {
    let schema = parse_json_schema(tool_name, val)?;
    let description = val
        .get("description")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let default = val.get("default").cloned();

    Ok(PropertySchema {
        schema,
        description,
        default,
    })
}

/// Formats a `TypeSchema` into its compact DSL string representation.
pub fn format_type_schema(schema: &TypeSchema) -> String {
    match schema {
        TypeSchema::String => "string".to_string(),
        TypeSchema::Integer => "integer".to_string(),
        TypeSchema::Number => "number".to_string(),
        TypeSchema::Boolean => "boolean".to_string(),
        TypeSchema::Null => "null".to_string(),
        TypeSchema::Enum(variants) => {
            let formatted: Vec<String> = variants
                .iter()
                .map(|v| format!("\"{}\"", v.replace('"', "\\\"")))
                .collect();
            formatted.join(" | ")
        }
        TypeSchema::Array(inner) => {
            let inner_str = format_type_schema(inner);
            if matches!(**inner, TypeSchema::Enum(_)) {
                format!("({})[]", inner_str)
            } else {
                format!("{}[]", inner_str)
            }
        }
        TypeSchema::Object {
            properties,
            required,
            ..
        } => {
            if properties.is_empty() {
                "object".to_string()
            } else {
                let mut out = String::from("{ ");
                let mut first = true;
                for (name, prop) in properties {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(name);
                    if !required.contains(name) {
                        out.push('?');
                    }
                    out.push_str(": ");
                    out.push_str(&format_type_schema(&prop.schema));
                }
                out.push_str(" }");
                out
            }
        }
    }
}
