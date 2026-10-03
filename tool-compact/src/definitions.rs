use crate::{
    error::DefinitionError,
    types::{CompactTools, ToolDef},
};
use serde_json::{Map, Value};

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, crate::EncodeError> {
    crate::encode::encode_tools(tools)
}

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, DefinitionError> {
    decode_definitions(compact.definitions())
}

pub fn decode_definitions(text: &str) -> Result<Vec<ToolDef>, DefinitionError> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_definition_line(trimmed, idx + 1)?);
    }
    Ok(out)
}

pub fn decode(text: &str, tools: &[ToolDef]) -> Result<crate::Decoded, crate::DecodeError> {
    let calls = decode_calls(text, tools)?;
    Ok(crate::Decoded {
        text: text.to_string(),
        calls,
    })
}

pub fn decode_calls(
    text: &str,
    tools: &[ToolDef],
) -> Result<Vec<crate::ToolCall>, crate::DecodeError> {
    let mut decoder = crate::StreamDecoder::new(tools)?;
    let events = decoder.push(text)?;
    let tail = decoder.finish()?;
    let mut collected = Vec::new();
    for event in events.into_iter().chain(tail) {
        if let crate::StreamEvent::Call { call, .. } = event {
            collected.push(call);
        }
    }
    Ok(collected)
}

fn parse_definition_line(line: &str, line_no: usize) -> Result<ToolDef, DefinitionError> {
    let (head, desc) = if let Some((before, after)) = line.split_once(" - ") {
        (before, Some(after.trim()))
    } else {
        (line, None)
    };

    let name_and_body = head.trim();
    let description = desc.map(str::to_string);

    if let Some(open) = name_and_body.find('(') {
        let end = match name_and_body.rfind(')') {
            Some(pos) => pos,
            None => name_and_body.len(),
        };
        if end <= open {
            return Err(DefinitionError {
                line: line_no,
                column: 1,
                reason: format!("malformed tool definition: {line}"),
            });
        }
        let (name_part, rest) = name_and_body.split_at(open);
        let name = name_part.trim();
        let inner = rest.get(1..end.saturating_sub(open - 1)).unwrap_or("");
        let required = parse_fields(inner, line_no)?;
        let parameters = if required.is_empty() {
            None
        } else {
            Some(Value::Object(required))
        };
        let tool = ToolDef {
            name: name.to_string(),
            description,
            parameters,
        };
        if tool.name.is_empty() {
            return Err(DefinitionError {
                line: line_no,
                column: 1,
                reason: "tool name is required".to_string(),
            });
        }
        return Ok(tool);
    }

    let name = name_and_body.trim();
    if name.is_empty() {
        return Err(DefinitionError {
            line: line_no,
            column: 1,
            reason: "empty tool name".to_string(),
        });
    }

    Ok(ToolDef {
        name: name.to_string(),
        description,
        parameters: None,
    })
}

fn parse_fields(inner: &str, line_no: usize) -> Result<Map<String, Value>, DefinitionError> {
    let mut fields: Map<String, Value> = Map::new();
    if inner.trim().is_empty() {
        return Ok(fields);
    }
    for part in split_top_level_commas(inner) {
        let entry = part.trim();
        if entry.is_empty() {
            continue;
        }
        let (left, right) = entry.split_once(':').ok_or_else(|| DefinitionError {
            line: line_no,
            column: 1,
            reason: format!("expected ':' in field: {entry}"),
        })?;
        let field_name = left.trim();
        let mut required = true;
        let field_name = if let Some(name) = field_name.strip_suffix('?') {
            required = false;
            name.trim()
        } else {
            field_name
        };
        if field_name.is_empty() {
            return Err(DefinitionError {
                line: line_no,
                column: 1,
                reason: "field name is empty".to_string(),
            });
        }
        let value_type = right.trim();
        let schema = parse_compact_node(value_type, line_no)?;
        let mut property: Map<String, Value> = Map::new();
        property.insert("type".to_string(), schema_type_for_value(&schema));
        if let Some(desc) = schema.get("description") {
            property.insert("description".to_string(), desc.clone());
        }
        fields.insert(field_name.to_string(), Value::Object(property));
        if required {
            // required list will be populated by caller in encode; not maintained here.
        }
    }
    Ok(fields)
}

fn split_top_level_commas(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (idx, ch) in input.char_indices() {
        match ch {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                if depth > 0 {
                    depth = depth.saturating_sub(1);
                }
            }
            ',' if depth == 0 => {
                if let Some(piece) = input.get(start..idx) {
                    parts.push(piece);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    if let Some(piece) = input.get(start..) {
        parts.push(piece);
    }
    parts
}

fn parse_compact_node(node: &str, line_no: usize) -> Result<Value, DefinitionError> {
    let trimmed = node.trim();
    if trimmed.is_empty() {
        return Err(DefinitionError {
            line: line_no,
            column: 1,
            reason: "empty node".to_string(),
        });
    }
    let normalized = trimmed.trim_matches('"').trim_matches('\'').trim();
    let base = if let Some(value) = normalized.strip_suffix("|null") {
        let mut obj: Map<String, Value> = Map::new();
        obj.insert(
            "type".to_string(),
            Value::Array(vec![
                Value::String(parse_type(value)?),
                Value::String("null".to_string()),
            ]),
        );
        return Ok(Value::Object(obj));
    } else {
        parse_type(normalized)?
    };
    Ok(Value::String(base))
}

fn parse_type(value: &str) -> Result<String, DefinitionError> {
    let trimmed = value.trim();
    match trimmed {
        "str" | "string" => Ok("string".to_string()),
        "int" | "integer" => Ok("integer".to_string()),
        "num" | "number" => Ok("number".to_string()),
        "bool" | "boolean" => Ok("boolean".to_string()),
        "null" => Ok("null".to_string()),
        "any" => Ok("any".to_string()),
        "obj" => Ok("object".to_string()),
        "datetime" => Ok("string".to_string()),
        "date" => Ok("string".to_string()),
        "time" => Ok("string".to_string()),
        "email" => Ok("string".to_string()),
        "uri" => Ok("string".to_string()),
        "uuid" => Ok("string".to_string()),
        _ if trimmed.starts_with('[') && trimmed.ends_with(']') => Ok("array".to_string()),
        _ if trimmed.contains('|') => Ok("enum".to_string()),
        _ => Ok(trimmed.to_string()),
    }
}

fn schema_type_for_value(value: &Value) -> Value {
    match value {
        Value::String(s) if s == "string" => Value::String("string".to_string()),
        Value::String(s) if s == "integer" => Value::String("integer".to_string()),
        Value::String(s) if s == "number" => Value::String("number".to_string()),
        Value::String(s) if s == "boolean" => Value::String("boolean".to_string()),
        Value::String(s) if s == "object" => Value::String("object".to_string()),
        Value::String(s) if s == "array" => Value::String("array".to_string()),
        Value::String(s) if s == "null" => Value::String("null".to_string()),
        Value::Array(items) if items.len() == 2 && items[0].is_string() => {
            Value::Array(items.clone())
        }
        _ => Value::String("any".to_string()),
    }
}
