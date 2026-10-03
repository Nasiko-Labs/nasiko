//! Compact text back to schemas and tool calls.
//!
//! `decode_tools` reads the signature text. It does not return the originals
//! stored on [`CompactTools`], so a renderer that drops a type fails the test.

use serde_json::{Map, Value};

use crate::encode::INSTRUCTION;
use crate::grammar;
use crate::schema::{self, Field, Scalar, Shape};
use crate::types::{CompactError, ToolCall, ToolDef};

pub(crate) fn schemas_from_text(text: &str) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line == INSTRUCTION {
            continue;
        }
        tools.push(parse_signature(line)?);
    }
    Ok(tools)
}

fn parse_signature(line: &str) -> Result<ToolDef, CompactError> {
    let (name, rest) = split_name(line)?;
    let (params, description) = split_params_and_description(&rest)?;
    let fields = parse_fields(&params)?;
    let description = if description.is_empty() {
        None
    } else {
        Some(description)
    };
    Ok(ToolDef {
        name,
        description,
        parameters: Some(object_schema(&fields)),
    })
}

fn split_name(line: &str) -> Result<(String, String), CompactError> {
    let mut name = String::new();
    let mut chars = line.chars();
    for ch in chars.by_ref() {
        if ch == '(' {
            return Ok((name, chars.collect()));
        }
        name.push(ch);
    }
    Err(malformed())
}

fn split_params_and_description(rest: &str) -> Result<(String, String), CompactError> {
    let mut params = String::new();
    let mut depth = 0i32;
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ')' && depth == 0 {
            let mut tail = String::new();
            tail.extend(chars);
            let description = tail
                .strip_prefix(" - ")
                .ok_or_else(malformed)?
                .to_string();
            return Ok((params, description));
        }
        match ch {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
        params.push(ch);
    }
    Err(malformed())
}

fn parse_fields(body: &str) -> Result<Vec<Field>, CompactError> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top(body, ',')
        .into_iter()
        .map(|part| parse_field(&part))
        .collect()
}

fn parse_field(raw: &str) -> Result<Field, CompactError> {
    let raw = raw.trim();
    let (name, required, ty) = split_mark(raw)?;
    Ok(Field {
        name: name.trim().to_string(),
        required,
        shape: parse_shape(ty.trim())?,
    })
}

/// Split `name:type` or `name?:type` on the mark at depth 0.
/// A nested `rooms?:int` must not steal the mark from `place:{...}`.
fn split_mark(raw: &str) -> Result<(String, bool, String), CompactError> {
    let chars: Vec<char> = raw.chars().collect();
    let mut depth = 0i32;
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            '?' if depth == 0 && chars.get(index + 1) == Some(&':') => {
                return Ok((
                    chars[..index].iter().collect(),
                    false,
                    chars[index + 2..].iter().collect(),
                ));
            }
            ':' if depth == 0 => {
                return Ok((
                    chars[..index].iter().collect(),
                    true,
                    chars[index + 1..].iter().collect(),
                ));
            }
            _ => {}
        }
        index += 1;
    }
    Err(malformed())
}

fn parse_shape(token: &str) -> Result<Shape, CompactError> {
    if let Some(inner) = token.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(Shape::Array(Box::new(parse_shape(inner.trim())?)));
    }
    if let Some(inner) = token.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        return Ok(Shape::Object {
            fields: parse_fields(inner)?,
        });
    }
    if token.contains('|') {
        return Ok(Shape::Enum(
            token.split('|').map(str::to_string).collect(),
        ));
    }
    Ok(match token {
        "str" => Shape::Scalar(Scalar::Str),
        "int" => Shape::Scalar(Scalar::Int),
        "num" => Shape::Scalar(Scalar::Num),
        "bool" => Shape::Scalar(Scalar::Bool),
        "datetime" => Shape::Scalar(Scalar::DateTime),
        _ => return Err(malformed()),
    })
}

fn split_top(input: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for ch in input.chars() {
        if ch == sep && depth == 0 {
            parts.push(std::mem::take(&mut current));
            continue;
        }
        if matches!(ch, '{' | '[') {
            depth += 1;
        } else if matches!(ch, '}' | ']') {
            depth -= 1;
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn object_schema(fields: &[Field]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in fields {
        if field.required {
            required.push(Value::String(field.name.clone()));
        }
        properties.insert(field.name.clone(), shape_to_value(&field.shape));
    }
    let mut object = Map::new();
    object.insert("type".into(), Value::String("object".into()));
    object.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        object.insert("required".into(), Value::Array(required));
    }
    Value::Object(object)
}

fn shape_to_value(shape: &Shape) -> Value {
    match shape {
        Shape::Scalar(Scalar::Str) => serde_json::json!({"type": "string"}),
        Shape::Scalar(Scalar::Int) => serde_json::json!({"type": "integer"}),
        Shape::Scalar(Scalar::Num) => serde_json::json!({"type": "number"}),
        Shape::Scalar(Scalar::Bool) => serde_json::json!({"type": "boolean"}),
        Shape::Scalar(Scalar::DateTime) => {
            serde_json::json!({"type": "string", "format": "date-time"})
        }
        Shape::Enum(values) => serde_json::json!({"type": "string", "enum": values}),
        Shape::Array(inner) => serde_json::json!({"type": "array", "items": shape_to_value(inner)}),
        Shape::Object { fields } => object_schema(fields),
    }
}

fn malformed() -> CompactError {
    grammar::malformed("")
}

pub(crate) fn calls_from_text(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut rest = text.to_string();
    let mut calls = Vec::new();
    loop {
        let Some((_, after_marker)) = rest.split_once("<<call ") else {
            break;
        };
        let after_marker = after_marker.to_string();
        let (raw, consumed) = match grammar::take_call(&after_marker)? {
            grammar::Taken::Incomplete => return Err(grammar::malformed("")),
            grammar::Taken::Ready(raw, consumed) => (raw, consumed),
        };
        calls.push(accept_call(raw, tools)?);
        rest = after_marker.chars().skip(consumed).collect();
    }
    if rest.contains("<<call") {
        return Err(grammar::malformed(""));
    }
    Ok(calls)
}

pub(crate) fn accept_call(raw: grammar::RawCall, tools: &[ToolDef]) -> Result<ToolCall, CompactError> {
    let Some(tool) = tools.iter().find(|tool| tool.name == raw.name) else {
        return Err(CompactError::UnknownTool { name: raw.name });
    };
    let parsed: Value =
        serde_json::from_str(&raw.arguments).map_err(|_| grammar::malformed(&raw.name))?;
    if !parsed.is_object() {
        return Err(grammar::malformed(&raw.name));
    }
    let shape = schema::classify(tool)?;
    schema::check(&shape, &parsed).map_err(|reason| CompactError::InvalidArguments {
        name: raw.name.clone(),
        reason,
    })?;
    Ok(ToolCall {
        name: raw.name,
        arguments: raw.arguments,
    })
}

