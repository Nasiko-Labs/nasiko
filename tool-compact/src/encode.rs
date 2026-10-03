//! Encoder: `Vec<ToolDef>` → [`CompactTools`].
//!
//! # Compact grammar (v1.0)
//!
//! ```text
//! ToolName(field1:type, field2?:type, arr?:[type], enumField?:a|b|c) - Description.
//! ```
//!
//! - Required fields: no suffix. Optional fields: `?` suffix.
//! - Types: `str`, `num`, `int`, `bool`, `[T]` (array), `{...}` (nested object).
//! - Enums: `a|b|c` inline (pipe-separated).
//! - Descriptions shortened but preserved where they disambiguate.
//!
//! # Bypass rules
//!
//! A tool is **bypassed** (full JSON schema sent) when its parameters contain:
//! - `oneOf`, `anyOf`, or `allOf` at the top level
//! - Nested objects deeper than 3 levels
//! - `$ref` pointers (not resolved here)

use std::collections::HashMap;
use std::fmt::Write;

use serde_json::Value;

use crate::compact_id::CompactId;
use crate::error::EncodeError;
use crate::types::{CompactTools, FieldType, ToolDef, ToolSchema};
use crate::GRAMMAR_VERSION;

/// Encode a slice of tool definitions into a compact textual representation.
///
/// Returns a [`CompactTools`] containing the compact text (suitable for injection
/// into a system message) and schema metadata for decode-time validation.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut schemas = Vec::with_capacity(tools.len());
    let mut text = format!(
        "# Available tools (compact schema v{GRAMMAR_VERSION}):\n"
    );

    for tool in tools {
        let name = &tool.function.name;
        if name.is_empty() {
            return Err(EncodeError::MissingName);
        }

        let schema = analyze_schema(tool);
        if schema.bypassed {
            // Bypass: include full JSON schema verbatim.
            writeln!(
                text,
                "[FULL SCHEMA] {name}: {}",
                serde_json::to_string(&tool.function.parameters)
                    .unwrap_or_else(|_| "null".into())
            )
            .unwrap();
            if let Some(desc) = &tool.function.description {
                writeln!(text, "  Description: {desc}").unwrap();
            }
        } else {
            // Compact representation.
            write!(text, "{name}(").unwrap();
            let fields = build_field_list(&schema);
            text.push_str(&fields);
            text.push(')');

            if let Some(desc) = &schema.description {
                let short = shorten_description(desc);
                write!(text, " - {short}").unwrap();
            }
            text.push('\n');
        }

        schemas.push(schema);
    }

    text.push('\n');
    text.push_str(
        "To call a tool, emit: <<call ToolName {\"field\":\"value\"}>>\n\
         Multiple calls: <<call T1 {...}>> <<call T2 {...}>>\n\
         String values containing >> must use \\u003e\\u003e.\n",
    );

    let compact_id = Some(CompactId::from_tools(tools).0);

    Ok(CompactTools {
        text,
        schemas,
        compact_id,
    })
}

/// Analyze a tool's JSON Schema parameters into a [`ToolSchema`].
fn analyze_schema(tool: &ToolDef) -> ToolSchema {
    let name = tool.function.name.clone();
    let description = tool.function.description.clone();
    let original_parameters = tool.function.parameters.clone();

    let Some(params) = &tool.function.parameters else {
        return ToolSchema {
            name,
            required_fields: vec![],
            optional_fields: vec![],
            field_types: HashMap::new(),
            enum_values: HashMap::new(),
            original_parameters,
            description,
            bypassed: false,
        };
    };

    // Check for bypass conditions.
    if should_bypass(params, 0) {
        return ToolSchema {
            name,
            required_fields: vec![],
            optional_fields: vec![],
            field_types: HashMap::new(),
            enum_values: HashMap::new(),
            original_parameters,
            description,
            bypassed: true,
        };
    }

    let required: Vec<String> = params
        .get("required")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let properties = params
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut field_types = HashMap::new();
    let mut enum_values = HashMap::new();
    let mut optional_fields = Vec::new();

    for (field_name, prop_schema) in &properties {
        let ft = infer_field_type(prop_schema);
        if let FieldType::Enum(ref vals) = ft {
            enum_values.insert(field_name.clone(), vals.clone());
        }
        field_types.insert(field_name.clone(), ft);

        if !required.contains(field_name) {
            optional_fields.push(field_name.clone());
        }
    }

    ToolSchema {
        name,
        required_fields: required,
        optional_fields,
        field_types,
        enum_values,
        original_parameters,
        description,
        bypassed: false,
    }
}

/// Decide whether a schema should be bypassed (too complex for compact encoding).
fn should_bypass(schema: &Value, depth: usize) -> bool {
    if depth > 3 {
        return true;
    }
    let obj = match schema.as_object() {
        Some(o) => o,
        None => return false,
    };

    // Top-level combinators cannot be faithfully compacted.
    if obj.contains_key("oneOf") || obj.contains_key("anyOf") || obj.contains_key("allOf") {
        return true;
    }
    if obj.contains_key("$ref") {
        return true;
    }

    // Recurse into properties.
    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for prop in props.values() {
            if should_bypass(prop, depth + 1) {
                return true;
            }
        }
    }
    // Recurse into array items.
    if let Some(items) = obj.get("items")
        && should_bypass(items, depth + 1)
    {
        return true;
    }

    false
}

/// Infer a [`FieldType`] from a JSON Schema property value.
fn infer_field_type(schema: &Value) -> FieldType {
    // Check for enum first — overrides type.
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let vals: Vec<String> = values
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();
        if !vals.is_empty() {
            return FieldType::Enum(vals);
        }
    }

    let type_str = schema
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("string");

    match type_str {
        "string" => FieldType::String,
        "number" => FieldType::Number,
        "integer" => FieldType::Integer,
        "boolean" => FieldType::Boolean,
        "array" => {
            let item_type = schema
                .get("items")
                .map(infer_field_type)
                .unwrap_or(FieldType::Unknown);
            FieldType::Array(Box::new(item_type))
        }
        "object" => FieldType::Object,
        _ => FieldType::Unknown,
    }
}

/// Build the comma-separated field list for the compact representation.
fn build_field_list(schema: &ToolSchema) -> String {
    let mut parts = Vec::new();

    // Required fields first (stable order), then optional.
    for field in &schema.required_fields {
        let type_str = schema
            .field_types
            .get(field)
            .map(|t| t.to_string())
            .unwrap_or_else(|| "any".into());
        parts.push(format!("{field}:{type_str}"));
    }
    for field in &schema.optional_fields {
        let type_str = schema
            .field_types
            .get(field)
            .map(|t| t.to_string())
            .unwrap_or_else(|| "any".into());
        parts.push(format!("{field}?:{type_str}"));
    }

    parts.join(", ")
}

/// Shorten a description for the compact representation.
///
/// Keeps the first sentence (up to the first period) and caps at 120 chars.
fn shorten_description(desc: &str) -> String {
    let trimmed = desc.trim();
    // Take up to the first sentence-ending period.
    let first_sentence = trimmed
        .find(". ")
        .map(|i| &trimmed[..=i])
        .unwrap_or(trimmed);

    if first_sentence.len() <= 120 {
        first_sentence.to_string()
    } else {
        format!("{}…", &first_sentence[..117])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_tool(name: &str, desc: Option<&str>, params: Option<Value>) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: crate::types::FunctionDef {
                name: name.into(),
                description: desc.map(String::from),
                parameters: params,
            },
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn encode_simple_tool() {
        let tools = vec![make_tool(
            "search",
            Some("Search the web."),
            Some(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            })),
        )];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("search(query:str) - Search the web."));
        assert!(compact.text.contains("<<call"));
        assert_eq!(compact.schemas.len(), 1);
        assert!(!compact.schemas[0].bypassed);
    }

    #[test]
    fn encode_tool_with_optional_and_enum() {
        let tools = vec![make_tool(
            "translate",
            Some("Translate text between languages."),
            Some(json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "target_lang": { "type": "string", "enum": ["en", "fr", "de", "es"] },
                    "formal": { "type": "boolean" }
                },
                "required": ["text", "target_lang"]
            })),
        )];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("text:str"));
        assert!(compact.text.contains("target_lang:en|fr|de|es"));
        assert!(compact.text.contains("formal?:bool"));
    }

    #[test]
    fn encode_tool_with_array() {
        let tools = vec![make_tool(
            "batch",
            None,
            Some(json!({
                "type": "object",
                "properties": {
                    "items": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["items"]
            })),
        )];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("items:[str]"));
    }

    #[test]
    fn encode_bypasses_oneof() {
        let tools = vec![make_tool(
            "complex",
            Some("Complex tool."),
            Some(json!({
                "type": "object",
                "properties": {
                    "input": {
                        "oneOf": [
                            { "type": "string" },
                            { "type": "integer" }
                        ]
                    }
                }
            })),
        )];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("[FULL SCHEMA]"));
        assert!(compact.schemas[0].bypassed);
    }

    #[test]
    fn encode_bypasses_deep_nesting() {
        let tools = vec![make_tool(
            "deep",
            None,
            Some(json!({
                "type": "object",
                "properties": {
                    "a": {
                        "type": "object",
                        "properties": {
                            "b": {
                                "type": "object",
                                "properties": {
                                    "c": {
                                        "type": "object",
                                        "properties": {
                                            "d": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            })),
        )];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.schemas[0].bypassed);
    }

    #[test]
    fn encode_empty_tools() {
        let compact = encode_tools(&[]).unwrap();
        assert!(compact.schemas.is_empty());
        assert!(compact.text.contains("compact schema"));
    }

    #[test]
    fn encode_tool_no_params() {
        let tools = vec![make_tool("ping", Some("Ping the server."), None)];
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("ping()"));
        assert!(!compact.schemas[0].bypassed);
    }

    #[test]
    fn encode_missing_name_is_error() {
        let tools = vec![make_tool("", None, None)];
        assert!(encode_tools(&tools).is_err());
    }

    #[test]
    fn compact_id_is_generated() {
        let tools = vec![make_tool("test", None, None)];
        let compact = encode_tools(&tools).unwrap();
        let id = compact.compact_id.as_ref().unwrap();
        assert!(id.starts_with("CT-"));
    }
}
