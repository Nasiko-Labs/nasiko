//! Compact tool schemas for LLM tool calling.
//!
//! Tool definitions cost prompt tokens on every request. This crate renders them in a
//! compact one-line-per-tool notation plus a single call-format instruction, and decodes
//! the model's compact call markers back into standard tool calls — validated against the
//! original JSON Schema, **fail-closed** (an unknown tool, missing required field, or
//! schema violation is an error, never a guessed call).
//!
//! # Grammar
//!
//! ```text
//! tool_line := name "(" [params] ")" " - " description "\n"
//! params    := param (", " param)*
//! param     := name ["?"] ":" type
//! type      := scalar | array | object | enum
//! scalar    := "str" | "int" | "num" | "bool" | "datetime"
//! array     := "[" type "]"
//! object    := "{" params "}"
//! enum      := literal ("|" literal)+
//!
//! call      := "<<call" ws name ws json ">>"
//! json      := JSON object (arguments)
//! ```
//!
//! The call terminator is the first `>>` **outside a JSON string literal**, so
//! `"a >> b"` inside a string argument does not end the call. `StreamDecoder` is
//! incremental: it buffers a partial marker when chunks split it.
//!
//! # Guarantees
//!
//! * Deterministic: pure string/value processing. No clock, no RNG, no I/O, no network.
//! * Fail-closed: `decode_calls` returns an error rather than a guessed or altered call.
//! * Round-trippable: `decode_tools(encode_tools(t))` reproduces the schema semantics.
//!
//! Schema features outside the supported subset (`anyOf`/`oneOf`/`$ref`, enums of
//! non-strings, objects without explicit `properties`, ...) make `encode_tools` fail;
//! callers must then bypass compaction and send the native request.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ── ToolDef / ToolCall (mirrors of the router types, converted at the seam) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn function_kind() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// A decoded, schema-validated tool call. `arguments` is a JSON object (the router
/// re-serializes it to the OpenAI string form at the seam).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// The compact rendering of a tool set: the text to inject, plus the defs it was
/// derived from so `decode_tools` can prove nothing was lost.
#[derive(Debug, Clone)]
pub struct CompactTools {
    pub text: String,
    defs: Vec<ToolDef>,
}

impl CompactTools {
    pub fn tool_defs(&self) -> &[ToolDef] {
        &self.defs
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("unsupported schema feature: {0}")]
    Unsupported(String),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("malformed call: {0}")]
    Malformed(String),
}

impl CompactError {
    /// Stable machine-readable kind used in eval output.
    pub fn kind(&self) -> &'static str {
        match self {
            CompactError::Unsupported(_) => "unsupported",
            CompactError::UnknownTool(_) => "unknown_tool",
            CompactError::InvalidArguments(_) => "invalid_arguments",
            CompactError::Malformed(_) => "invalid_arguments",
        }
    }
}

// ── Encoding: JSON Schema → compact line ─────────────────────────────────────

/// Instruction line appended to every compact rendering (part of the contract).
pub const CALL_INSTRUCTION: &str = "To call a tool, emit: <<call name {json args}>>";

pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, CompactError> {
    let mut text = String::new();
    for t in tools {
        let params = encode_params(t.function.parameters.as_ref())?;
        text.push_str(&t.function.name);
        text.push('(');
        text.push_str(&params);
        text.push(')');
        if let Some(desc) = &t.function.description {
            text.push_str(" - ");
            text.push_str(desc);
        }
        text.push('\n');
    }
    text.push_str(CALL_INSTRUCTION);
    text.push('\n');
    Ok(CompactTools {
        text,
        defs: tools.to_vec(),
    })
}

fn encode_params(schema: Option<&Value>) -> Result<String, CompactError> {
    let Some(schema) = schema else {
        return Ok(String::new());
    };
    encode_object(schema).map(|(body, _)| body)
}

/// Returns (compact body, required-prop-names in order).
fn encode_object(schema: &Value) -> Result<(String, Vec<String>), CompactError> {
    let props = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| CompactError::Unsupported("object without properties".into()))?;
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut parts = Vec::with_capacity(props.len());
    for (name, subschema) in props {
        let t = encode_type(subschema)?;
        let optional = if required.iter().any(|r| r == name) {
            ""
        } else {
            "?"
        };
        parts.push(format!("{name}{optional}:{t}"));
    }
    Ok((parts.join(", "), required))
}

fn encode_type(schema: &Value) -> Result<String, CompactError> {
    // enum first (may not have a scalar type of interest)
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array) {
        let mut lits = Vec::with_capacity(enum_vals.len());
        for v in enum_vals {
            match v {
                Value::String(s) => lits.push(s.clone()),
                _ => return Err(CompactError::Unsupported("non-string enum".into())),
            }
        }
        let t = schema.get("type").and_then(Value::as_str);
        if t.is_some() && t != Some("string") {
            return Err(CompactError::Unsupported("enum on non-string type".into()));
        }
        return Ok(lits.join("|"));
    }
    if schema.get("anyOf").is_some()
        || schema.get("oneOf").is_some()
        || schema.get("allOf").is_some()
    {
        return Err(CompactError::Unsupported("anyOf/oneOf/allOf".into()));
    }
    if schema.get("$ref").is_some() {
        return Err(CompactError::Unsupported("$ref".into()));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            if schema.get("format").and_then(Value::as_str) == Some("date-time") {
                Ok("datetime".into())
            } else {
                Ok("str".into())
            }
        }
        Some("integer") => Ok("int".into()),
        Some("number") => Ok("num".into()),
        Some("boolean") => Ok("bool".into()),
        Some("array") => {
            let items = schema
                .get("items")
                .ok_or_else(|| CompactError::Unsupported("array without items".into()))?;
            Ok(format!("[{}]", encode_type(items)?))
        }
        Some("object") => {
            if schema.get("properties").is_some() {
                let (body, _) = encode_object(schema)?;
                Ok(format!("{{{body}}}"))
            } else {
                Err(CompactError::Unsupported(
                    "free-form object (no properties)".into(),
                ))
            }
        }
        other => Err(CompactError::Unsupported(format!("type {other:?}"))),
    }
}

// ── Decoding: compact text → ToolDef list (schema preservation check) ────────

pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, CompactError> {
    decode_tools_text(&compact.text)
}

fn decode_tools_text(text: &str) -> Result<Vec<ToolDef>, CompactError> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line == CALL_INSTRUCTION {
            continue;
        }
        out.push(decode_tool_line(line)?);
    }
    Ok(out)
}

fn decode_tool_line(line: &str) -> Result<ToolDef, CompactError> {
    let open = line
        .find('(')
        .ok_or_else(|| CompactError::Malformed(format!("no params in {line:?}")))?;
    let close = line
        .find(')')
        .filter(|&c| c > open)
        .ok_or_else(|| CompactError::Malformed(format!("unbalanced parens in {line:?}")))?;
    let name = line[..open].trim().to_string();
    let params_src = &line[open + 1..close];
    let desc = line[close + 1..].strip_prefix(" - ").map(|d| d.to_string());
    let (properties, required) = decode_params(params_src)?;
    let parameters = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });
    Ok(ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name,
            description: desc,
            parameters: Some(parameters),
        },
        extra: Map::new(),
    })
}

fn decode_params(src: &str) -> Result<(Map<String, Value>, Vec<Value>), CompactError> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    let src = src.trim();
    if src.is_empty() {
        return Ok((properties, required));
    }
    for part in split_top_level(src, ',') {
        let part = part.trim();
        let name_end = part
            .find(['?', ':'])
            .ok_or_else(|| CompactError::Malformed(format!("bad param {part:?}")))?;
        let raw_name = part[..name_end].trim();
        let (name, optional) = (
            raw_name.trim_end_matches('?'),
            part[name_end..].starts_with('?'),
        );
        let type_src = part[name_end..]
            .trim_start_matches('?')
            .trim_start_matches(':');
        let schema = decode_type(type_src.trim())?;
        if !optional {
            required.push(Value::String(name.to_string()));
        }
        properties.insert(name.to_string(), schema);
    }
    Ok((properties, required))
}

/// Split on `sep` at depth 0 of `( ) { } [ ]` (respects no string literals — types have none).
fn split_top_level(src: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in src.char_indices() {
        match c {
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&src[start..]);
    parts
}

fn decode_type(src: &str) -> Result<Value, CompactError> {
    let alts: Vec<&str> = src.split('|').collect();
    if alts.len() > 1 {
        let lits: Vec<Value> = alts
            .iter()
            .map(|a| Value::String(a.trim().to_string()))
            .collect();
        return Ok(serde_json::json!({"type": "string", "enum": lits}));
    }
    let src = src.trim();
    if let Some(inner) = src.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(serde_json::json!({"type": "array", "items": decode_type(inner)?}));
    }
    if let Some(inner) = src.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let (properties, required) = decode_params(inner)?;
        return Ok(serde_json::json!({
            "type": "object", "properties": properties, "required": required
        }));
    }
    Ok(match src {
        "str" => serde_json::json!({"type": "string"}),
        "datetime" => serde_json::json!({"type": "string", "format": "date-time"}),
        "int" => serde_json::json!({"type": "integer"}),
        "num" => serde_json::json!({"type": "number"}),
        "bool" => serde_json::json!({"type": "boolean"}),
        other if is_enum_literal(other) => {
            serde_json::json!({"type": "string", "enum": [other]})
        }
        other => return Err(CompactError::Malformed(format!("unknown type {other:?}"))),
    })
}

fn is_enum_literal(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ── Decoding: model output → validated tool calls ────────────────────────────

const START: &str = "<<call";

/// One-shot decode of a full model response. Plain text around (or without any)
/// call markers is ignored; every call marker present is parsed and validated.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut dec = StreamDecoder::new(tools);
    dec.push(text)?;
    dec.finish()
}

/// Incremental decoder. Feed chunks as they arrive; `finish` validates and
/// returns every call (or the first error, fail-closed).
#[derive(Debug, Default)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    calls: Vec<ToolCall>,
    err: Option<CompactError>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        StreamDecoder {
            tools: tools.to_vec(),
            ..Default::default()
        }
    }

    /// Buffer a chunk; returns calls completed by it. On the first error the
    /// decoder sticks to it (fail-closed).
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(e) = &self.err {
            // Keep the original error kind (unknown_tool stays unknown_tool).
            return Err(match e {
                CompactError::UnknownTool(s) => CompactError::UnknownTool(s.clone()),
                CompactError::InvalidArguments(s) => CompactError::InvalidArguments(s.clone()),
                CompactError::Malformed(s) => CompactError::Malformed(s.clone()),
                CompactError::Unsupported(s) => CompactError::Unsupported(s.clone()),
            });
        }
        self.buf.push_str(chunk);
        let mut fresh = Vec::new();
        loop {
            match self.extract_one() {
                Extract::Call(call) => match validate_call(&call, &self.tools) {
                    Ok(()) => {
                        self.calls.push(call.clone());
                        fresh.push(call);
                    }
                    Err(e) => {
                        self.err = Some(e);
                        self.buf.clear();
                        return Err(self.err_as());
                    }
                },
                Extract::Fail(e) => {
                    self.err = Some(e);
                    self.buf.clear();
                    return Err(self.err_as());
                }
                Extract::NeedMore => break,
                Extract::None => break,
            }
        }
        Ok(fresh)
    }

    fn err_as(&self) -> CompactError {
        match self.err.as_ref().unwrap() {
            CompactError::UnknownTool(s) => CompactError::UnknownTool(s.clone()),
            CompactError::InvalidArguments(s) => CompactError::InvalidArguments(s.clone()),
            CompactError::Malformed(s) => CompactError::Malformed(s.clone()),
            CompactError::Unsupported(s) => CompactError::Unsupported(s.clone()),
        }
    }

    /// Flush: an unterminated call marker is an error (fail-closed); other
    /// trailing text is a plain answer and yields no calls.
    pub fn finish(mut self) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(e) = self.err.take() {
            return Err(e);
        }
        // Anything left that looks like a call start is an unterminated call.
        if self.buf.contains(START) {
            return Err(CompactError::Malformed("unterminated call marker".into()));
        }
        Ok(self.calls)
    }

    /// Try to consume one complete or decided call from `buf`.
    fn extract_one(&mut self) -> Extract {
        let Some(pos) = self.buf.find(START) else {
            // Keep a possible split marker prefix ("<<", "<<c", ...).
            let keep = longest_marker_prefix_suffix(&self.buf);
            self.buf = self.buf[self.buf.len() - keep..].to_string();
            return Extract::None;
        };
        // Drop text before the marker.
        if pos > 0 {
            self.buf.drain(..pos);
        }
        match parse_call(&self.buf) {
            ParseOutcome::Complete(len, call) => {
                self.buf.drain(..len);
                Extract::Call(call)
            }
            ParseOutcome::Incomplete => Extract::NeedMore,
            ParseOutcome::Bad(e) => Extract::Fail(e),
        }
    }
}

enum Extract {
    Call(ToolCall),
    Fail(CompactError),
    NeedMore,
    None,
}

enum ParseOutcome {
    Complete(usize, ToolCall),
    Incomplete,
    Bad(CompactError),
}

/// Length of the longest suffix of `s` that is a proper prefix of `<<call`.
fn longest_marker_prefix_suffix(s: &str) -> usize {
    let max = (START.len() - 1).min(s.len());
    for k in (1..=max).rev() {
        if START.starts_with(&s[s.len() - k..]) {
            return k;
        }
    }
    0
}

/// Parse `<<call name {json}>>` at the start of `buf` (buf[0] is '<').
/// `>>` only terminates outside a JSON string literal.
fn parse_call(buf: &str) -> ParseOutcome {
    let after_start = &buf[START.len()..];
    let rest = after_start.trim_start();
    if rest.is_empty() {
        return ParseOutcome::Incomplete;
    }
    let name_end = rest
        .find(|c: char| c.is_whitespace() || c == '{')
        .unwrap_or(rest.len());
    if name_end == 0 {
        return ParseOutcome::Bad(CompactError::Malformed("missing tool name".into()));
    }
    if name_end == rest.len() {
        // name may be truncated; we cannot know yet unless more arrives — but a name
        // followed by end-of-buffer is ambiguous: it might be the full name with the
        // args still coming. Only resolvable with more input.
        return ParseOutcome::Incomplete;
    }
    let name = &rest[..name_end];
    let after_name = rest[name_end..].trim_start();
    if after_name.is_empty() {
        return ParseOutcome::Incomplete;
    }
    if !after_name.starts_with('{') {
        return ParseOutcome::Bad(CompactError::Malformed("expected { after name".into()));
    }
    match scan_json_object(after_name) {
        JsonScan::Complete(json_len) => {
            let args_src = &after_name[..json_len];
            let after_json = after_name[json_len..].trim_start();
            if after_json.is_empty() {
                return ParseOutcome::Incomplete; // maybe closing >> still coming
            }
            if let Some(rest2) = after_json.strip_prefix(">>") {
                let end = buf.len() - rest2.len();
                match serde_json::from_str::<Value>(args_src) {
                    Ok(arguments) => ParseOutcome::Complete(
                        end,
                        ToolCall {
                            name: name.to_string(),
                            arguments,
                        },
                    ),
                    Err(_) => ParseOutcome::Bad(CompactError::InvalidArguments(
                        "arguments are not valid JSON".into(),
                    )),
                }
            } else if after_json.len() == 1 && after_json.starts_with('>') {
                ParseOutcome::Incomplete // possibly split ">>"
            } else {
                ParseOutcome::Bad(CompactError::Malformed(
                    "expected >> after arguments".into(),
                ))
            }
        }
        JsonScan::Incomplete => ParseOutcome::Incomplete,
        JsonScan::Bad => ParseOutcome::Bad(CompactError::InvalidArguments(
            "arguments are not valid JSON".into(),
        )),
    }
}

enum JsonScan {
    Complete(usize),
    Incomplete,
    Bad,
}

/// Find the end of a JSON object starting at `s[0] == '{'`, string-aware.
fn scan_json_object(s: &str) -> JsonScan {
    debug_assert!(s.starts_with('{'));
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return JsonScan::Complete(i + 1);
                }
                if depth < 0 {
                    return JsonScan::Bad;
                }
            }
            _ => {}
        }
    }
    // Decide complete vs bad: still inside a string or unbalanced → incomplete
    // (more bytes may close it); anything structurally odd is caught by serde.
    if depth > 0 || in_str {
        JsonScan::Incomplete
    } else {
        JsonScan::Bad
    }
}

// ── Schema validation (fail-closed) ──────────────────────────────────────────

fn validate_call(call: &ToolCall, tools: &[ToolDef]) -> Result<(), CompactError> {
    let def = tools
        .iter()
        .find(|t| t.function.name == call.name)
        .ok_or_else(|| CompactError::UnknownTool(call.name.clone()))?;
    let schema = match &def.function.parameters {
        Some(s) => s,
        None => {
            // Tool takes no arguments: only {} is valid.
            return match call.arguments.as_object() {
                Some(o) if o.is_empty() => Ok(()),
                _ => Err(CompactError::InvalidArguments(
                    "expected no arguments".into(),
                )),
            };
        }
    };
    validate_value(&call.arguments, schema, &call.name)
}

fn validate_value(v: &Value, schema: &Value, path: &str) -> Result<(), CompactError> {
    let bad = |msg: &str| CompactError::InvalidArguments(format!("{path}: {msg}"));
    if let Some(enum_vals) = schema.get("enum").and_then(Value::as_array)
        && !enum_vals.iter().any(|e| e == v)
    {
        return Err(bad("value not in enum"));
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => match v {
            Value::String(_) => Ok(()),
            _ => Err(bad("expected string")),
        },
        Some("integer") => match v {
            Value::Number(n) if n.is_i64() || n.is_u64() => Ok(()),
            _ => Err(bad("expected integer")),
        },
        Some("number") => match v {
            Value::Number(_) => Ok(()),
            _ => Err(bad("expected number")),
        },
        Some("boolean") => match v {
            Value::Bool(_) => Ok(()),
            _ => Err(bad("expected boolean")),
        },
        Some("array") => match v {
            Value::Array(items) => {
                if let Some(item_schema) = schema.get("items") {
                    for (i, item) in items.iter().enumerate() {
                        validate_value(item, item_schema, &format!("{path}[{i}]"))?;
                    }
                }
                Ok(())
            }
            _ => Err(bad("expected array")),
        },
        Some("object") => match v {
            Value::Object(map) => {
                // required
                if let Some(req) = schema.get("required").and_then(Value::as_array) {
                    for r in req {
                        if let Some(name) = r.as_str()
                            && !map.contains_key(name)
                        {
                            return Err(CompactError::InvalidArguments(format!(
                                "{path}: missing required field {name:?}"
                            )));
                        }
                    }
                }
                // per-property types
                if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                    for (k, sub) in props {
                        if let Some(val) = map.get(k) {
                            validate_value(val, sub, &format!("{path}.{k}"))?;
                        }
                    }
                }
                Ok(())
            }
            _ => Err(bad("expected object")),
        },
        None => Ok(()), // no type constraint (e.g. only enum) — enum already checked
        Some(other) => Err(CompactError::Unsupported(format!("type {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "create_calendar_event",
                "description": "Create an event in the user's calendar.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer"},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                }
            }
        }))
        .unwrap()
    }

    fn email_tool() -> ToolDef {
        serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "send_email",
                "description": "Send an email.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"}
                    },
                    "required": ["to", "subject", "body"]
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn encode_decode_roundtrip_preserves_schema() {
        let tools = vec![calendar_tool(), email_tool()];
        let compact = encode_tools(&tools).unwrap();
        let back = decode_tools(&compact).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].function.name, "create_calendar_event");
        let params = back[0].function.parameters.as_ref().unwrap();
        let req = params["required"].as_array().unwrap();
        assert_eq!(req.len(), 2);
        assert_eq!(
            params["properties"]["visibility"]["enum"],
            json!(["public", "private"])
        );
        assert_eq!(
            params["properties"]["attendees"]["items"]["type"],
            json!("string")
        );
        assert_eq!(params["properties"]["start"]["format"], json!("date-time"));
    }

    #[test]
    fn decode_valid_call() {
        let tools = vec![calendar_tool()];
        let calls = decode_calls(
            r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>>"#,
            &tools,
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert_eq!(calls[0].arguments["title"], json!("Retro"));
    }

    #[test]
    fn double_gt_inside_string_does_not_terminate() {
        let tools = vec![email_tool()];
        let calls = decode_calls(
            r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#,
            &tools,
        )
        .unwrap();
        assert_eq!(calls[0].arguments["subject"], json!("a >> b"));
    }

    #[test]
    fn unknown_tool_fails_closed() {
        let tools = vec![calendar_tool()];
        let e = decode_calls(r#"<<call delete_everything {}>>"#, &tools).unwrap_err();
        assert!(matches!(e, CompactError::UnknownTool(_)));
        assert_eq!(e.kind(), "unknown_tool");
    }

    #[test]
    fn missing_required_and_bad_enum_fail_closed() {
        let tools = vec![calendar_tool()];
        let e = decode_calls(
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
            &tools,
        )
        .unwrap_err();
        assert_eq!(e.kind(), "invalid_arguments");
    }

    #[test]
    fn wrong_type_fails_closed() {
        let tools = vec![calendar_tool()];
        let e = decode_calls(
            r#"<<call create_calendar_event {"title":"x","start":"y","duration_min":"thirty"}>>"#,
            &tools,
        )
        .unwrap_err();
        assert_eq!(e.kind(), "invalid_arguments");
    }

    #[test]
    fn marker_split_across_chunks() {
        let tools = vec![calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        for c in [
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ] {
            dec.push(c).unwrap();
        }
        let calls = dec.finish().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["title"], json!("Retro"));
    }

    #[test]
    fn multiple_calls_and_surrounding_text() {
        let tools = vec![calendar_tool(), email_tool()];
        let text = r#"Sure! <<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}>> and <<call create_calendar_event {"title":"t","start":"s"}>> done."#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "send_email");
        assert_eq!(calls[1].name, "create_calendar_event");
    }

    #[test]
    fn plain_answer_has_no_calls() {
        let tools = vec![calendar_tool()];
        let calls = decode_calls("What's the weather? No idea.", &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn unterminated_marker_is_an_error() {
        let tools = vec![calendar_tool()];
        let e = decode_calls(
            r#"<<call create_calendar_event {"title":"t","start":"s"}>"#,
            &tools,
        )
        .unwrap_err();
        assert_eq!(e.kind(), "invalid_arguments");
    }

    #[test]
    fn unsupported_schema_is_rejected_for_compaction() {
        let tool: ToolDef = serde_json::from_value(json!({
            "type": "function",
            "function": {
                "name": "x",
                "parameters": {"type": "object", "properties": {"a": {"anyOf": []}}}
            }
        }))
        .unwrap();
        assert!(matches!(
            encode_tools(&[tool]),
            Err(CompactError::Unsupported(_))
        ));
    }
}
