//! `decode_tools`: rebuild tool definitions from the compact text.
//!
//! Lets a checker confirm that schema meaning (types, required vs optional, enums, nesting,
//! descriptions) survived encoding. Only text produced by [`crate::encode_tools`] is accepted;
//! anything else is an error, never a guess.

use serde_json::{Map, Value, json};

use crate::types::{CompactTools, EncodeError, ToolDef};

/// Parse the signature blocks in `compact.text` back into tool definitions.
///
/// Bypassed tools are not in the text and are not returned.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, EncodeError> {
    let mut lines = compact.text.lines();
    if !lines.by_ref().any(|line| line == "Tools:") {
        return Err(bad("missing `Tools:` marker"));
    }
    let mut tools: Vec<ToolDef> = Vec::new();
    for line in lines {
        if let Some(note) = line.strip_prefix("  ") {
            let tool = tools
                .last_mut()
                .ok_or_else(|| bad("note before any tool"))?;
            apply_note(tool, note)?;
        } else {
            tools.push(parse_signature(line)?);
        }
    }
    Ok(tools)
}

fn bad(msg: &str) -> EncodeError {
    EncodeError::InvalidTool(format!("decode_tools: {msg}"))
}

/// `name(params) - description`
fn parse_signature(line: &str) -> Result<ToolDef, EncodeError> {
    let open = line.find('(').ok_or_else(|| bad("signature without `(`"))?;
    let close = line.find(')').ok_or_else(|| bad("signature without `)`"))?;
    if close < open {
        return Err(bad("malformed signature"));
    }
    let name = &line[..open];
    let params = &line[open + 1..close];
    let rest = &line[close + 1..];
    let description = match rest {
        "" => None,
        _ => Some(
            rest.strip_prefix(" - ")
                .ok_or_else(|| bad("text after `)` must be ` - description`"))?
                .to_string(),
        ),
    };
    let schema = object_schema(params)?;
    let has_props = schema.get("properties").is_some();
    Ok(ToolDef {
        name: name.to_string(),
        description,
        parameters: Some(if has_props {
            schema
        } else {
            json!({"type": "object"})
        }),
    })
}

/// Parse `k:T, k2?:T` into an object schema.
fn object_schema(text: &str) -> Result<Value, EncodeError> {
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for field in split_top(text, ',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let colon = field.find(':').ok_or_else(|| bad("field without `:`"))?;
        let (raw_name, ty) = (&field[..colon], &field[colon + 1..]);
        let (name, is_required) = match raw_name.strip_suffix('?') {
            Some(n) => (n, false),
            None => (raw_name, true),
        };
        if properties
            .insert(name.to_string(), type_schema(ty)?)
            .is_some()
        {
            return Err(bad("duplicate property"));
        }
        if is_required {
            required.push(json!(name));
        }
    }
    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    if !properties.is_empty() {
        schema.insert("properties".into(), Value::Object(properties));
    }
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    Ok(Value::Object(schema))
}

/// Split on `sep` at bracket depth 0.
fn split_top(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in text.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn type_schema(text: &str) -> Result<Value, EncodeError> {
    match text {
        "str" => return Ok(json!({"type": "string"})),
        "datetime" => return Ok(json!({"type": "string", "format": "date-time"})),
        "int" => return Ok(json!({"type": "integer"})),
        "num" => return Ok(json!({"type": "number"})),
        "bool" => return Ok(json!({"type": "boolean"})),
        _ => {}
    }
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return Ok(json!({"type": "array", "items": type_schema(inner)?}));
    }
    if let Some(inner) = text.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
        let mut schema = object_schema(inner)?;
        if schema.get("properties").is_none() {
            schema = json!({"type": "object", "properties": {}});
        }
        return Ok(schema);
    }
    if text.is_empty() || text.contains(['[', ']', '{', '}', ',', ':', '?', ' ']) {
        return Err(bad("malformed type"));
    }
    let values: Vec<Value> = text.split('|').map(|v| json!(v)).collect();
    if values.iter().any(|v| v.as_str() == Some("")) {
        return Err(bad("empty enum value"));
    }
    Ok(json!({"type": "string", "enum": values}))
}

/// `path: description` where path is `prop`, `a.b` or `list[].field`.
fn apply_note(tool: &mut ToolDef, note: &str) -> Result<(), EncodeError> {
    let (path, desc) = note
        .split_once(": ")
        .ok_or_else(|| bad("note without `: `"))?;
    let mut node = tool
        .parameters
        .as_mut()
        .ok_or_else(|| bad("note for a tool without parameters"))?;
    for segment in path.split('.') {
        let (name, arrays) = split_arrays(segment);
        node = node
            .get_mut("properties")
            .and_then(|p| p.get_mut(name))
            .ok_or_else(|| bad("note path does not match a property"))?;
        for _ in 0..arrays {
            node = node
                .get_mut("items")
                .ok_or_else(|| bad("note path crosses a non-array"))?;
        }
    }
    node["description"] = json!(desc);
    Ok(())
}

/// `rooms[][]` -> (`rooms`, 2)
fn split_arrays(segment: &str) -> (&str, usize) {
    let mut name = segment;
    let mut arrays = 0;
    while let Some(stripped) = name.strip_suffix("[]") {
        name = stripped;
        arrays += 1;
    }
    (name, arrays)
}
