//! Decoding: model text → validated [`ToolCall`]s, and the compact block → [`ToolDef`]s.
//!
//! The marker scanner ([`next_marker`]) is the shared primitive used both here (whole-string
//! decode) and by [`crate::StreamDecoder`] (incremental decode). It finds one `<<call ...>>`
//! at a time, matching the JSON object body by brace depth with full string/escape awareness
//! so a `>>` *inside* a string argument never ends the marker early.

use serde_json::{Map, Value};

use crate::{CompactError, CompactTools, Result, ToolCall, ToolDef};

const MARKER_OPEN: &str = "<<call";
const MARKER_CLOSE: &str = ">>";

/// One scan step over a slice. Indices are relative to the slice passed in.
pub(crate) enum Marker {
    /// A complete, well-formed marker: `args_text` is the raw JSON object between the braces
    /// (still to be parsed and schema-validated by [`validate_call`]). `end` is the index just
    /// past the closing `>>`.
    Call {
        end: usize,
        name: String,
        args_text: String,
    },
    /// A marker opened but is structurally broken and will never complete (e.g. a body that is
    /// not a JSON object). `end` is just past the recovered `>>` so scanning can continue.
    Malformed { end: usize, detail: String },
    /// A `<<call` opened but the slice ends before the marker closes — the caller should wait
    /// for more input (streaming) or stop (whole-string decode).
    Incomplete,
    /// No `<<call` remains in the slice.
    None,
}

/// Find the next tool-call marker in `s`, scanning from the start of the slice.
pub(crate) fn next_marker(s: &str) -> Marker {
    let bytes = s.as_bytes();
    let Some(open) = s.find(MARKER_OPEN) else {
        return Marker::None;
    };
    let mut pos = open + MARKER_OPEN.len();

    // Require whitespace between `<<call` and the tool name; `<<caller` is not a marker.
    match bytes.get(pos) {
        None => return Marker::Incomplete,
        Some(c) if c.is_ascii_whitespace() => {}
        Some(_) => {
            // False positive (e.g. "<<callback"); resume search past this `<<`.
            return match next_marker(&s[open + 2..]) {
                Marker::Call { end, name, args_text } => Marker::Call {
                    end: end + open + 2,
                    name,
                    args_text,
                },
                Marker::Malformed { end, detail } => Marker::Malformed {
                    end: end + open + 2,
                    detail,
                },
                other => other,
            };
        }
    }

    pos = skip_ws(bytes, pos);
    if pos >= bytes.len() {
        return Marker::Incomplete;
    }

    let name_start = pos;
    while pos < bytes.len() && is_name_byte(bytes[pos]) {
        pos += 1;
    }
    let name = s[name_start..pos].to_string();
    if pos >= bytes.len() {
        return Marker::Incomplete;
    }
    if name.is_empty() {
        return recover_malformed(s, open, "missing tool name".to_string());
    }

    pos = skip_ws(bytes, pos);
    if pos >= bytes.len() {
        return Marker::Incomplete;
    }
    if bytes[pos] != b'{' {
        return recover_malformed(s, open, "arguments must be a JSON object".to_string());
    }

    let Some(close_brace) = json_object_end(bytes, pos) else {
        return Marker::Incomplete; // object not balanced yet — wait for more input
    };
    let args_text = s[pos..=close_brace].to_string();

    let mut after = skip_ws(bytes, close_brace + 1);
    if after + MARKER_CLOSE.len() > bytes.len() {
        // Not enough bytes to confirm `>>` yet; if what we have is a prefix of `>>`, wait.
        if s[after..].is_empty() || MARKER_CLOSE.starts_with(&s[after..]) {
            return Marker::Incomplete;
        }
        return recover_malformed(s, open, "expected `>>` after arguments".to_string());
    }
    if &s[after..after + MARKER_CLOSE.len()] == MARKER_CLOSE {
        after += MARKER_CLOSE.len();
        return Marker::Call { end: after, name, args_text };
    }
    recover_malformed(s, open, "expected `>>` after arguments".to_string())
}

/// Recover from a broken marker by finding the next `>>` so scanning can move past it. If no
/// `>>` has arrived yet, the marker is still [`Marker::Incomplete`].
fn recover_malformed(s: &str, open: usize, detail: String) -> Marker {
    match s[open..].find(MARKER_CLOSE) {
        Some(rel) => Marker::Malformed {
            end: open + rel + MARKER_CLOSE.len(),
            detail,
        },
        None => Marker::Incomplete,
    }
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'
}

/// Index of the `}` that closes the `{` at `start`, or `None` if the object is not yet
/// balanced within `bytes`. String-aware: braces inside JSON strings and escaped quotes are
/// ignored, so `>>` or `{`/`}` inside an argument value never confuses the scan.
fn json_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Decode every tool call in `text`, validating each against the schemas in `tools`.
///
/// Fail-closed: the first unknown tool, unparseable argument payload, or schema violation
/// returns an error rather than a guessed call. Plain text with no marker yields an empty
/// `Vec`. A `<<call` that never closes (truncated input) is treated as "no more complete
/// calls" and ignored — use [`crate::StreamDecoder`] when input arrives in pieces.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let mut offset = 0usize;
    while offset < text.len() {
        match next_marker(&text[offset..]) {
            Marker::Call { end, name, args_text } => {
                calls.push(validate_call(&name, &args_text, tools)?);
                offset += end;
            }
            Marker::Malformed { detail, .. } => {
                return Err(CompactError::MalformedMarker(detail));
            }
            Marker::Incomplete | Marker::None => break,
        }
    }
    Ok(calls)
}

/// Parse and schema-validate a single call. The one place "fail closed" is enforced.
pub(crate) fn validate_call(name: &str, args_text: &str, tools: &[ToolDef]) -> Result<ToolCall> {
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| CompactError::UnknownTool(name.to_string()))?;

    let value: Value = serde_json::from_str(args_text)
        .map_err(|e| CompactError::MalformedArguments(e.to_string()))?;
    let args = value.as_object().ok_or_else(|| CompactError::InvalidArguments {
        tool: name.to_string(),
        reason: "arguments must be a JSON object".to_string(),
    })?;

    if let Some(schema) = tool.parameters.as_ref().and_then(Value::as_object) {
        validate_object(name, args, schema)?;
    }

    Ok(ToolCall {
        name: name.to_string(),
        arguments: value,
    })
}

/// Validate a top-level arguments object against a JSON Schema object: required fields present,
/// and every provided field that the schema names has a matching type / enum value. Unknown
/// extra fields are permitted (the schema may allow additional properties); the decoder's job
/// is to refuse *invalid* calls, not to strip unmodelled keys.
fn validate_object(tool: &str, args: &Map<String, Value>, schema: &Map<String, Value>) -> Result<()> {
    if let Some(Value::Array(required)) = schema.get("required") {
        for req in required {
            if let Some(field) = req.as_str() {
                if !args.contains_key(field) {
                    return Err(CompactError::InvalidArguments {
                        tool: tool.to_string(),
                        reason: format!("missing required argument `{field}`"),
                    });
                }
            }
        }
    }

    let props = schema.get("properties").and_then(Value::as_object);
    if let Some(props) = props {
        for (key, val) in args {
            if let Some(prop_schema) = props.get(key) {
                validate_value(tool, key, val, prop_schema)?;
            }
        }
    }
    Ok(())
}

/// Validate one value against one property schema: enum membership first (it constrains the
/// value directly), then the structural `type`. Arrays recurse into `items`; objects are
/// checked shallowly (nested object *shapes* are intentionally not enforced — see encode.rs).
fn validate_value(tool: &str, field: &str, val: &Value, schema: &Value) -> Result<()> {
    let Some(obj) = schema.as_object() else {
        return Ok(());
    };
    let err = |reason: String| CompactError::InvalidArguments {
        tool: tool.to_string(),
        reason,
    };

    if let Some(Value::Array(variants)) = obj.get("enum") {
        if !variants.iter().any(|v| v == val) {
            return Err(err(format!("`{field}` is not one of the allowed values")));
        }
        return Ok(());
    }

    let Some(ty) = obj.get("type").and_then(Value::as_str) else {
        return Ok(());
    };
    let ok = match ty {
        "string" => val.is_string(),
        "integer" => val.is_i64() || val.is_u64(),
        "number" => val.is_number(),
        "boolean" => val.is_boolean(),
        "object" => val.is_object(),
        "array" => val.is_array(),
        _ => true,
    };
    if !ok {
        return Err(err(format!("`{field}` must be of type {ty}")));
    }

    if ty == "array" {
        if let (Some(items), Some(arr)) = (obj.get("items"), val.as_array()) {
            for item in arr {
                validate_value(tool, field, item, items)?;
            }
        }
    }
    Ok(())
}

/// Recover [`ToolDef`]s from a [`CompactTools`] block — the optional reverse of
/// [`encode_tools`], so a harness can confirm schema information survived encoding.
///
/// Recovers names, required/optional split, scalar types, enums, and array element types.
/// Nested object shapes collapse to `{"type":"object"}` (documented loss). Property
/// descriptions are not recovered (encoding drops them); the function description is.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    let mut tools = Vec::new();
    for raw in compact.tools_block.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        tools.push(parse_tool_line(line)?);
    }
    Ok(tools)
}

fn parse_tool_line(line: &str) -> Result<ToolDef> {
    let open = line.find('(').ok_or_else(|| CompactError::MalformedMarker(
        format!("compact tool line missing `(`: {line}"),
    ))?;
    let name = line[..open].trim().to_string();

    // Match the `(` to its closing `)` by depth so array/enum parens don't confuse it. The
    // params never contain nested `(` in this grammar, so a simple scan from the right works:
    let close = line.rfind(')').ok_or_else(|| CompactError::MalformedMarker(
        format!("compact tool line missing `)`: {line}"),
    ))?;
    if close < open {
        return Err(CompactError::MalformedMarker(format!("unbalanced parens: {line}")));
    }
    let params = &line[open + 1..close];

    let description = line[close + 1..]
        .trim()
        .strip_prefix('-')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());

    let (properties, required) = parse_params(params);
    let parameters = if properties.is_empty() {
        Some(Value::Object(Map::new()))
    } else {
        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::String("object".to_string()));
        schema.insert("properties".to_string(), Value::Object(properties));
        if !required.is_empty() {
            schema.insert(
                "required".to_string(),
                Value::Array(required.into_iter().map(Value::String).collect()),
            );
        }
        Some(Value::Object(schema))
    };

    Ok(ToolDef { name, description, parameters })
}

fn parse_params(params: &str) -> (Map<String, Value>, Vec<String>) {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for part in split_top_level(params) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some(colon) = part.find(':') else { continue };
        let (mut name, type_tok) = (part[..colon].trim(), part[colon + 1..].trim());
        let optional = name.ends_with('?');
        if optional {
            name = &name[..name.len() - 1];
        } else {
            required.push(name.to_string());
        }
        properties.insert(name.to_string(), schema_for_type(type_tok));
    }
    (properties, required)
}

/// Split a param list on top-level commas, ignoring commas inside `[...]` or `a|b` groups
/// (arrays can nest, enums use `|` not `,`, so only brackets need depth tracking).
fn split_top_level(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_string());
    parts
}

fn schema_for_type(tok: &str) -> Value {
    let tok = tok.trim();
    if let Some(inner) = tok.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::String("array".to_string()));
        schema.insert("items".to_string(), schema_for_type(inner));
        return Value::Object(schema);
    }
    if tok.contains('|') {
        let variants: Vec<Value> = tok.split('|').map(|v| Value::String(v.trim().to_string())).collect();
        let mut schema = Map::new();
        schema.insert("enum".to_string(), Value::Array(variants));
        return Value::Object(schema);
    }
    let mut schema = Map::new();
    match tok {
        "str" => {
            schema.insert("type".to_string(), Value::String("string".to_string()));
        }
        "datetime" => {
            schema.insert("type".to_string(), Value::String("string".to_string()));
            schema.insert("format".to_string(), Value::String("date-time".to_string()));
        }
        "int" => {
            schema.insert("type".to_string(), Value::String("integer".to_string()));
        }
        "float" => {
            schema.insert("type".to_string(), Value::String("number".to_string()));
        }
        "bool" => {
            schema.insert("type".to_string(), Value::String("boolean".to_string()));
        }
        "obj" => {
            schema.insert("type".to_string(), Value::String("object".to_string()));
        }
        _ => {} // "any" / unknown → empty schema (no constraint)
    }
    Value::Object(schema)
}
