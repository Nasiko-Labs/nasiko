//! Encoder: tool definitions to compact signature lines.
//!
//! Format (one block per tool):
//!
//! ```text
//! name(req:T, opt?:T) - tool description
//!   prop: description of a property
//!   list[].field: description of a property nested inside an array of objects
//! ```
//!
//! Property order is deterministic: required properties in `required` order, then optional
//! properties alphabetically. Any schema feature outside the supported subset makes the tool
//! be reported in [`CompactTools::bypassed`] with a reason code; it is never guessed.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::types::{Bypass, CompactTools, EncodeError, ToolDef};

const HEADER: &str = "To call a tool, emit: <<call name {json args}>>\n\
Write calls as plain reply text, never via function calling, exactly like <<call get_x {\"a\":1}>>. Args: one JSON object; `?` optional; a|b allowed values. No tool needed: answer normally.\n\
Tools:";

/// Keywords the encoder understands at a schema node. Anything else bypasses the tool.
const SUPPORTED_KEYWORDS: [&str; 7] = [
    "type",
    "properties",
    "required",
    "enum",
    "items",
    "format",
    "description",
];

/// Render `tools` as call-format instructions plus one signature block per compactable tool.
/// Tools using schema features outside the supported subset are listed in `bypassed` and left
/// out of `text`; the caller decides whether to send the whole request natively.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, EncodeError> {
    let mut seen = HashSet::new();
    let mut blocks = Vec::new();
    let mut bypassed = Vec::new();
    for tool in tools {
        if !is_identifier(&tool.name, true) {
            return Err(EncodeError::InvalidTool(format!(
                "tool name {:?} is not a valid identifier",
                tool.name
            )));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err(EncodeError::InvalidTool(format!(
                "duplicate tool name {:?}",
                tool.name
            )));
        }
        match encode_tool(tool) {
            Ok(block) => blocks.push(block),
            Err(reason) => bypassed.push(Bypass {
                tool: tool.name.clone(),
                reason,
            }),
        }
    }
    let mut text = String::from(HEADER);
    for block in &blocks {
        text.push('\n');
        text.push_str(block);
    }
    Ok(CompactTools { text, bypassed })
}

/// Encode one tool, or return the bypass reason code.
fn encode_tool(tool: &ToolDef) -> Result<String, String> {
    let mut notes: Vec<(String, String)> = Vec::new();
    let params = match &tool.parameters {
        None => String::new(),
        Some(schema) => {
            let obj = schema
                .as_object()
                .ok_or_else(|| "unsupported:schema".to_string())?;
            check_keywords(obj)?;
            if obj.get("type").and_then(Value::as_str) != Some("object") {
                return Err("unsupported:type".into());
            }
            if obj.contains_key("description") {
                return Err("unsupported:description".into());
            }
            object_fields(obj, "", &mut notes, ", ")?
        }
    };
    let mut block = format!("{}({})", tool.name, params);
    if let Some(desc) = tool.description.as_deref().filter(|d| !d.trim().is_empty()) {
        check_single_line(desc)?;
        block.push_str(" - ");
        block.push_str(desc.trim());
    }
    for (path, desc) in notes {
        block.push_str("\n  ");
        block.push_str(&path);
        block.push_str(": ");
        block.push_str(&desc);
    }
    Ok(block)
}

fn check_single_line(text: &str) -> Result<(), String> {
    if text.contains('\n') || text.contains('\r') {
        Err("unsupported:description-newline".into())
    } else {
        Ok(())
    }
}

/// Validate the keywords of a schema node against the supported subset.
fn check_keywords(node: &Map<String, Value>) -> Result<(), String> {
    for key in node.keys() {
        if !SUPPORTED_KEYWORDS.contains(&key.as_str()) {
            return Err(format!("unsupported:{key}"));
        }
    }
    Ok(())
}

/// Render the `properties` of an object node as `k:T, k2?:T`, collecting property descriptions.
fn object_fields(
    node: &Map<String, Value>,
    path: &str,
    notes: &mut Vec<(String, String)>,
    sep: &str,
) -> Result<String, String> {
    check_keywords(node)?;
    if node.contains_key("enum") || node.contains_key("items") || node.contains_key("format") {
        return Err("unsupported:object-keyword".into());
    }
    let props = match node.get("properties") {
        None => return Ok(String::new()),
        Some(Value::Object(props)) => props,
        Some(_) => return Err("unsupported:properties".into()),
    };
    let required = required_names(node, props)?;
    let mut optional: Vec<&String> = props.keys().filter(|k| !required.contains(k)).collect();
    optional.sort();
    let ordered = required
        .iter()
        .map(|k| (k, true))
        .chain(optional.into_iter().map(|k| (k, false)));

    let mut parts = Vec::new();
    for (name, is_required) in ordered {
        if !is_identifier(name, false) {
            return Err("unsupported:property-name".into());
        }
        let child = props
            .get(name)
            .and_then(Value::as_object)
            .ok_or_else(|| "unsupported:schema".to_string())?;
        let child_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}.{name}")
        };
        if let Some(desc) = property_description(child)? {
            notes.push((child_path.clone(), desc));
        }
        let ty = type_text(child, &child_path, notes)?;
        parts.push(format!(
            "{}{}:{}",
            name,
            if is_required { "" } else { "?" },
            ty
        ));
    }
    Ok(parts.join(sep))
}

/// `required` entries in order; each must name a declared property.
fn required_names(
    node: &Map<String, Value>,
    props: &Map<String, Value>,
) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    match node.get("required") {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                let name = item
                    .as_str()
                    .ok_or_else(|| "unsupported:required".to_string())?;
                if !props.contains_key(name) || names.iter().any(|n| n == name) {
                    return Err("unsupported:required".into());
                }
                names.push(name.to_string());
            }
        }
        Some(_) => return Err("unsupported:required".into()),
    }
    Ok(names)
}

/// The `description` of a property node, if non-empty. Must be a single line.
fn property_description(node: &Map<String, Value>) -> Result<Option<String>, String> {
    match node.get("description") {
        None => Ok(None),
        Some(Value::String(text)) => {
            check_single_line(text)?;
            let trimmed = text.trim();
            Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
        }
        Some(_) => Err("unsupported:description".into()),
    }
}

/// Compact type text for a schema node.
fn type_text(
    node: &Map<String, Value>,
    path: &str,
    notes: &mut Vec<(String, String)>,
) -> Result<String, String> {
    check_keywords(node)?;
    if let Some(values) = node.get("enum") {
        return enum_text(node, values);
    }
    let ty = node
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "unsupported:type".to_string())?;
    if node.contains_key("format") && ty != "string" {
        return Err("unsupported:format".into());
    }
    match ty {
        "string" => match node.get("format") {
            None => Ok("str".into()),
            Some(Value::String(f)) if f == "date-time" => Ok("datetime".into()),
            Some(_) => Err("unsupported:format".into()),
        },
        "integer" => scalar(node, "int"),
        "number" => scalar(node, "num"),
        "boolean" => scalar(node, "bool"),
        "array" => {
            if node.contains_key("properties") || node.contains_key("required") {
                return Err("unsupported:array-keyword".into());
            }
            let items = node
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| "unsupported:items".to_string())?;
            if items.contains_key("description") {
                return Err("unsupported:description".into());
            }
            let inner = type_text(items, &format!("{path}[]"), notes)?;
            Ok(format!("[{inner}]"))
        }
        "object" => {
            if !node.contains_key("properties") {
                return Err("unsupported:properties".into());
            }
            let inner = object_fields(node, path, notes, ",")?;
            Ok(format!("{{{inner}}}"))
        }
        _ => Err("unsupported:type".into()),
    }
}

/// Scalar types take no structural keywords.
fn scalar(node: &Map<String, Value>, text: &str) -> Result<String, String> {
    for key in ["properties", "required", "items", "format"] {
        if node.contains_key(key) {
            return Err(format!("unsupported:{key}"));
        }
    }
    Ok(text.to_string())
}

/// `a|b|c` for a non-empty enum of simple strings.
fn enum_text(node: &Map<String, Value>, values: &Value) -> Result<String, String> {
    match node.get("type") {
        None => {}
        Some(Value::String(t)) if t == "string" => {}
        Some(_) => return Err("unsupported:enum".into()),
    }
    for key in ["properties", "required", "items", "format"] {
        if node.contains_key(key) {
            return Err(format!("unsupported:{key}"));
        }
    }
    let list = values
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "unsupported:enum".to_string())?;
    let mut tokens = Vec::with_capacity(list.len());
    for value in list {
        let token = value
            .as_str()
            .filter(|t| is_enum_token(t))
            .ok_or_else(|| "unsupported:enum-value".to_string())?;
        tokens.push(token);
    }
    // A lone value spelled like a type word would read back as that type.
    if let [only] = tokens.as_slice()
        && RESERVED_TYPE_WORDS.contains(only)
    {
        return Err("unsupported:enum-value".into());
    }
    Ok(tokens.join("|"))
}

/// Words the compact type syntax already uses.
const RESERVED_TYPE_WORDS: [&str; 5] = ["str", "int", "num", "bool", "datetime"];

fn is_enum_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+'))
}

/// Tool names allow `.`; property names do not, so dotted note paths stay unambiguous.
fn is_identifier(name: &str, allow_dot: bool) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || (allow_dot && c == '.'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool(name: &str, desc: Option<&str>, params: Option<Value>) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: desc.map(String::from),
            parameters: params,
        }
    }

    fn calendar() -> ToolDef {
        tool(
            "create_calendar_event",
            Some("Create an event in the user's calendar."),
            Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                    "duration_min": {"type": "integer", "description": "Duration in minutes"},
                    "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        )
    }

    fn only_block(tools: &[ToolDef]) -> String {
        let out = encode_tools(tools).unwrap();
        assert!(out.bypassed.is_empty(), "{:?}", out.bypassed);
        out.text
            .strip_prefix(HEADER)
            .unwrap()
            .trim_start_matches('\n')
            .to_string()
    }

    #[test]
    fn briefs_calendar_example_keeps_required_first_and_types() {
        let block = only_block(&[calendar()]);
        assert_eq!(
            block,
            "create_calendar_event(title:str, start:datetime, attendees?:[str], duration_min?:int, \
visibility?:public|private) - Create an event in the user's calendar.\n  \
title: Event title\n  start: Start time, ISO 8601\n  attendees: Attendee emails\n  \
duration_min: Duration in minutes"
        );
    }

    #[test]
    fn text_starts_with_call_format_instruction() {
        let out = encode_tools(&[calendar()]).unwrap();
        assert!(
            out.text
                .starts_with("To call a tool, emit: <<call name {json args}>>\n")
        );
    }

    #[test]
    fn nested_objects_arrays_and_their_descriptions() {
        let t = tool(
            "book",
            None,
            Some(json!({
                "type": "object",
                "properties": {
                    "guest": {"type": "object", "properties": {
                        "name": {"type": "string", "description": "Full name"},
                        "age": {"type": "integer"}
                    }, "required": ["name"]},
                    "rooms": {"type": "array", "items": {"type": "object", "properties": {
                        "beds": {"type": "number", "description": "Bed count"}
                    }, "required": ["beds"]}}
                },
                "required": ["guest"]
            })),
        );
        assert_eq!(
            only_block(&[t]),
            "book(guest:{name:str,age?:int}, rooms?:[{beds:num}])\n  guest.name: Full name\n  rooms[].beds: Bed count"
        );
    }

    #[test]
    fn no_parameters_and_empty_schema() {
        assert_eq!(
            only_block(&[tool("ping", Some("Ping."), None)]),
            "ping() - Ping."
        );
        let t = tool("now", None, Some(json!({"type": "object"})));
        assert_eq!(only_block(&[t]), "now()");
    }

    #[test]
    fn unsupported_keywords_bypass_with_reason_codes() {
        let cases = [
            (json!({"$ref": "#/defs/x"}), "unsupported:$ref"),
            (
                json!({"type": "object", "properties": {"a": {"oneOf": [{"type": "string"}]}}}),
                "unsupported:oneOf",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "integer", "minimum": 1}}}),
                "unsupported:minimum",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "x"}}}),
                "unsupported:pattern",
            ),
            (
                json!({"type": "object", "additionalProperties": false}),
                "unsupported:additionalProperties",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": ["string", "null"]}}}),
                "unsupported:type",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "const": "x"}}}),
                "unsupported:const",
            ),
            (
                json!({"type": "object", "properties": {"a b": {"type": "string"}}}),
                "unsupported:property-name",
            ),
            (
                json!({"type": "object", "properties": {"a": {"enum": ["int"]}}}),
                "unsupported:enum-value",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "enum": ["a:b", "c"]}}}),
                "unsupported:enum-value",
            ),
            (
                json!({"type": "object", "properties": {"a": {"enum": [1, 2]}}}),
                "unsupported:enum-value",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string", "description": "x\ny"}}}),
                "unsupported:description-newline",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "array"}}}),
                "unsupported:items",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "object"}}}),
                "unsupported:properties",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["b"]}),
                "unsupported:required",
            ),
        ];
        for (schema, reason) in cases {
            let out = encode_tools(&[tool("t", None, Some(schema.clone()))]).unwrap();
            assert_eq!(out.bypassed.len(), 1, "{schema}");
            assert_eq!(out.bypassed[0].reason, reason, "{schema}");
            assert_eq!(out.bypassed[0].tool, "t");
            assert!(
                !out.text.contains("\nt("),
                "bypassed tool must not be in text"
            );
        }
    }

    #[test]
    fn one_bypassed_tool_does_not_hide_the_others_in_the_report() {
        let bad = tool("bad", None, Some(json!({"$ref": "#/x"})));
        let out = encode_tools(&[calendar(), bad]).unwrap();
        assert_eq!(out.bypassed.len(), 1);
        assert!(out.text.contains("create_calendar_event("));
    }

    #[test]
    fn invalid_and_duplicate_names_are_errors() {
        assert!(encode_tools(&[tool("bad name", None, None)]).is_err());
        assert!(encode_tools(&[tool("", None, None)]).is_err());
        assert!(encode_tools(&[tool("a", None, None), tool("a", None, None)]).is_err());
    }

    #[test]
    fn encoding_is_deterministic() {
        let a = encode_tools(&[calendar()]).unwrap();
        let b = encode_tools(&[calendar()]).unwrap();
        assert_eq!(a, b);
    }
}
