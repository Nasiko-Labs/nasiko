use crate::error::CompactError;
use crate::types::{CompactTools, ToolDef};
use serde_json::{Map, Value, json};

/// Instructions injected alongside the compact tool block.
pub const CALL_INSTRUCTIONS: &str = "\
CALL FORMAT
To call a tool, emit exactly:
<<call tool_name {\"arg\": \"value\"}>>
Rules:
- The payload after the tool name is one JSON object holding the arguments.
- Emit one <<call ...>> block per call; emit several blocks to make several calls.
- Arguments marked ? are optional; omit them when not needed.
- Never invent an argument that is not listed, and never invent a tool name.
- Values after : must match the listed type. a|b means one of those exact strings.
- If no tool applies, answer in plain text and emit no <<call ...>> block.";

/// Keys this format does not represent. Their presence means the caller should
/// bypass compaction for the request.
const UNSUPPORTED_KEYS: [&str; 6] =
    ["oneOf", "anyOf", "allOf", "not", "$ref", "patternProperties"];

fn guard(schema: &Value) -> Result<(), CompactError> {
    if let Some(obj) = schema.as_object() {
        for k in UNSUPPORTED_KEYS {
            if obj.contains_key(k) {
                return Err(CompactError::Unsupported { reason: format!("`{k}` is not representable") });
            }
        }
    }
    Ok(())
}

fn unsupported(reason: &str) -> CompactError {
    CompactError::Unsupported { reason: reason.to_string() }
}

/// Render a tool set compactly. Returns `Unsupported` if any tool uses a schema
/// feature this format cannot carry; callers then bypass compaction entirely so
/// that behaviour stays all-or-nothing per request.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    if tools.is_empty() {
        return Err(unsupported("no tools to encode"));
    }
    let mut lines = Vec::with_capacity(tools.len());
    for t in tools {
        lines.push(encode_one(t)?);
    }
    Ok(CompactTools {
        tools_block: format!("TOOLS\n{}", lines.join("\n")),
        instructions: CALL_INSTRUCTIONS.to_string(),
    })
}

fn encode_one(tool: &ToolDef) -> Result<String, CompactError> {
    let params = tool.parameters.clone().unwrap_or_else(|| json!({"type": "object"}));
    guard(&params)?;
    let obj = params.as_object().ok_or_else(|| unsupported("parameters is not an object"))?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err(unsupported("root schema must be type object"));
    }

    let required = required_set(obj);
    let mut sig = Vec::new();
    let mut notes = Vec::new();

    if let Some(props) = obj.get("properties").and_then(Value::as_object) {
        for (name, schema) in props {
            let opt = if required.iter().any(|r| r == name) { "" } else { "?" };
            sig.push(format!("{name}{opt}:{}", render_type(schema)?));
            if let Some(d) = schema.get("description").and_then(Value::as_str) {
                notes.push(format!("  @{name}: {d}"));
            }
        }
    }

    let mut line = format!("{}({})", tool.name, sig.join(", "));
    if let Some(d) = &tool.description {
        line.push_str(&format!(" - {d}"));
    }
    if !notes.is_empty() {
        line.push('\n');
        line.push_str(&notes.join("\n"));
    }
    Ok(line)
}

fn required_set(obj: &Map<String, Value>) -> Vec<String> {
    obj.get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn render_type(schema: &Value) -> Result<String, CompactError> {
    guard(schema)?;
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        if e.is_empty() {
            return Err(unsupported("empty enum"));
        }
        let vals: Result<Vec<String>, CompactError> = e
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| unsupported("only string enums are representable"))
            })
            .collect();
        return Ok(vals?.join("|"));
    }

    let ty = schema.get("type").and_then(Value::as_str).ok_or_else(|| unsupported("missing type"))?;
    Ok(match ty {
        "string" => {
            if schema.get("format").and_then(Value::as_str) == Some("date-time") {
                "datetime".to_string()
            } else {
                "str".to_string()
            }
        }
        "integer" => "int".to_string(),
        "number" => "num".to_string(),
        "boolean" => "bool".to_string(),
        "array" => {
            let items = schema.get("items").ok_or_else(|| unsupported("array without items"))?;
            format!("[{}]", render_type(items)?)
        }
        "object" => {
            let obj = schema.as_object().ok_or_else(|| unsupported("bad object schema"))?;
            let required = required_set(obj);
            let props =
                obj.get("properties").and_then(Value::as_object).ok_or_else(|| unsupported("object without properties"))?;
            let mut inner = Vec::new();
            for (k, v) in props {
                let opt = if required.iter().any(|r| r == k) { "" } else { "?" };
                inner.push(format!("{k}{opt}:{}", render_type(v)?));
            }
            format!("{{{}}}", inner.join(", "))
        }
        other => return Err(unsupported(&format!("type `{other}`"))),
    })
}

/// Write a call in the compact grammar. Used by the eval harness to render
/// expected calls, and by tests.
pub fn render_call(name: &str, arguments: &Value) -> String {
    format!("<<call {name} {}>>", serde_json::to_string(arguments).unwrap_or_else(|_| "{}".into()))
}

// ---------------------------------------------------------------------------
// Inverse: compact text back to JSON Schema, so schema survival is checkable.
// ---------------------------------------------------------------------------

/// Rebuild tool definitions from a compact block. The encoding is lossless, so
/// this returns schemas equal to the originals.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    let mut tools: Vec<ToolDef> = Vec::new();
    for raw in compact.tools_block.lines() {
        if raw.trim().is_empty() || raw.trim() == "TOOLS" {
            continue;
        }
        if let Some(note) = raw.strip_prefix("  @") {
            let (arg, desc) = note
                .split_once(": ")
                .ok_or_else(|| CompactError::MalformedCall { reason: "bad @note line".into() })?;
            let tool =
                tools.last_mut().ok_or_else(|| CompactError::MalformedCall { reason: "note before tool".into() })?;
            if let Some(p) = tool
                .parameters
                .as_mut()
                .and_then(|p| p.get_mut("properties"))
                .and_then(Value::as_object_mut)
                .and_then(|m| m.get_mut(arg))
                .and_then(Value::as_object_mut)
            {
                p.insert("description".into(), Value::String(desc.to_string()));
            }
            continue;
        }
        tools.push(parse_signature(raw)?);
    }
    Ok(tools)
}

fn parse_signature(line: &str) -> Result<ToolDef, CompactError> {
    let open = line.find('(').ok_or_else(|| CompactError::MalformedCall { reason: "no (".into() })?;
    let name = line[..open].trim().to_string();
    let close = matching(line, open).ok_or_else(|| CompactError::MalformedCall { reason: "no )".into() })?;
    let sig = &line[open + 1..close];
    let description = line[close + 1..].trim().strip_prefix("- ").map(str::to_string);

    let mut props = Map::new();
    let mut required = Vec::new();
    for part in split_top(sig, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let colon =
            top_index(part, ':').ok_or_else(|| CompactError::MalformedCall { reason: "arg without :".into() })?;
        let (mut key, ty) = (part[..colon].trim(), part[colon + 1..].trim());
        if let Some(stripped) = key.strip_suffix('?') {
            key = stripped;
        } else {
            required.push(Value::String(key.to_string()));
        }
        props.insert(key.to_string(), parse_type(ty)?);
    }

    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    Ok(ToolDef { name, description, parameters: Some(Value::Object(schema)) })
}

fn parse_type(t: &str) -> Result<Value, CompactError> {
    let t = t.trim();
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(json!({"type": "array", "items": parse_type(inner)?}));
    }
    if let Some(inner) = t.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut props = Map::new();
        let mut required = Vec::new();
        for part in split_top(inner, ',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let colon =
                top_index(part, ':').ok_or_else(|| CompactError::MalformedCall { reason: "field without :".into() })?;
            let (mut key, ty) = (part[..colon].trim(), part[colon + 1..].trim());
            if let Some(stripped) = key.strip_suffix('?') {
                key = stripped;
            } else {
                required.push(Value::String(key.to_string()));
            }
            props.insert(key.to_string(), parse_type(ty)?);
        }
        let mut schema = Map::new();
        schema.insert("type".into(), Value::String("object".into()));
        schema.insert("properties".into(), Value::Object(props));
        if !required.is_empty() {
            schema.insert("required".into(), Value::Array(required));
        }
        return Ok(Value::Object(schema));
    }
    if t.contains('|') {
        let vals: Vec<Value> = t.split('|').map(|v| Value::String(v.trim().to_string())).collect();
        return Ok(json!({"type": "string", "enum": vals}));
    }
    Ok(match t {
        "str" => json!({"type": "string"}),
        "datetime" => json!({"type": "string", "format": "date-time"}),
        "int" => json!({"type": "integer"}),
        "num" => json!({"type": "number"}),
        "bool" => json!({"type": "boolean"}),
        other => return Err(unsupported(&format!("unknown compact type `{other}`"))),
    })
}

/// Index of the matching closing bracket for the opener at `open`.
fn matching(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on `sep` at bracket depth zero.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Byte index of the first `needle` at bracket depth zero.
fn top_index(s: &str, needle: char) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            _ => {}
        }
        if c == needle && depth == 0 {
            return Some(i);
        }
    }
    None
}
