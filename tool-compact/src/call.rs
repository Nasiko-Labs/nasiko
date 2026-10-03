//! Compact call syntax: `<<call name {json}>>`.
//!
//! Arguments stay JSON so nested values, escapes, and unicode use a real parser.
//! The marker is only recognized when `<<call` is followed by whitespace, which
//! keeps words like `<<calling` in ordinary text.

use serde_json::Value;

use crate::cursor::Cur;
use crate::error::Error;
use crate::schema::{self, is_tool_name, validate};
use crate::types::{ToolCall, ToolDef};

/// One completed piece from [`StreamDecoder`].
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    Text(String),
    Call(ToolCall),
}

/// Incremental decoder. Markers may split at any byte the chunk boundary hits,
/// including inside the marker, the name, a JSON string, or `>>`.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    next_index: usize,
    failed: Option<Error>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            buf: String::new(),
            next_index: 0,
            failed: None,
        }
    }

    /// Feed the next chunk. Completed text and calls are returned in order.
    /// An incomplete marker stays buffered.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamItem>, Error> {
        if let Some(err) = &self.failed {
            return Err(err.clone());
        }
        self.buf.push_str(chunk);
        self.drain().inspect_err(|err| self.fail(err.clone()))
    }

    /// End of stream. A committed but unfinished call is an error. A dangling
    /// prefix such as `<<` is ordinary text, because no call was started.
    pub fn finish(&mut self) -> Result<Vec<StreamItem>, Error> {
        if let Some(err) = &self.failed {
            return Err(err.clone());
        }
        let mut items = self.drain().inspect_err(|err| self.fail(err.clone()))?;
        if self.buf.is_empty() {
            return Ok(items);
        }
        if tail_committed(&self.buf) {
            let err = Error::Malformed("truncated compact call".into());
            self.fail(err.clone());
            return Err(err);
        }
        let rest = std::mem::take(&mut self.buf);
        push_text(&mut items, rest);
        Ok(items)
    }

    fn fail(&mut self, err: Error) {
        self.failed = Some(err);
    }

    fn drain(&mut self) -> Result<Vec<StreamItem>, Error> {
        let mut items = Vec::new();
        loop {
            match pull(&self.buf, &self.tools, self.next_index)? {
                Pull::NeedMore => break,
                Pull::Text(bytes) => {
                    let text: String = self.buf.drain(..bytes).collect();
                    push_text(&mut items, text);
                }
                Pull::Call { consumed, call } => {
                    self.buf.drain(..consumed);
                    self.next_index += 1;
                    items.push(StreamItem::Call(call));
                }
            }
        }
        Ok(items)
    }
}

fn push_text(items: &mut Vec<StreamItem>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(StreamItem::Text(existing)) = items.last_mut() {
        existing.push_str(&text);
    } else {
        items.push(StreamItem::Text(text));
    }
}

/// `<<call` plus whitespace means a call has started and must finish.
fn tail_committed(buf: &str) -> bool {
    let Some(rest) = buf.strip_prefix("<<call") else {
        return false;
    };
    rest.chars().next().is_some_and(char::is_whitespace)
}

enum Pull {
    NeedMore,
    Text(usize),
    Call { consumed: usize, call: ToolCall },
}

fn pull(buf: &str, tools: &[ToolDef], index: usize) -> Result<Pull, Error> {
    let mut i = 0;
    while i < buf.len() {
        let ch = buf[i..].chars().next().unwrap_or('\0');
        if ch != '<' {
            i += ch.len_utf8();
            continue;
        }
        let rest = &buf[i..];
        if let Some(after) = rest.strip_prefix("<<call") {
            match after.chars().next() {
                None => {
                    return Ok(if i == 0 {
                        Pull::NeedMore
                    } else {
                        Pull::Text(i)
                    });
                }
                Some(next) if !next.is_whitespace() => {
                    i += ch.len_utf8();
                    continue;
                }
                Some(_) => {
                    if i > 0 {
                        return Ok(Pull::Text(i));
                    }
                    return parse_call(buf, tools, index);
                }
            }
        }
        if "<<call".starts_with(rest) {
            return Ok(if i == 0 {
                Pull::NeedMore
            } else {
                Pull::Text(i)
            });
        }
        i += ch.len_utf8();
    }
    if buf.is_empty() {
        Ok(Pull::NeedMore)
    } else {
        Ok(Pull::Text(buf.len()))
    }
}

fn parse_call(buf: &str, tools: &[ToolDef], index: usize) -> Result<Pull, Error> {
    let mut cur = Cur::new(buf);
    if !cur.eat_str("<<call") {
        return Err(Error::Malformed("internal marker mismatch".into()));
    }
    if !cur.skip_ws() {
        return Ok(Pull::NeedMore);
    }
    let name_start = cur.i;
    if cur.eof() {
        return Ok(Pull::NeedMore);
    }
    if cur.peek().is_some_and(char::is_whitespace) {
        return Err(Error::Malformed("missing tool name".into()));
    }
    while cur.peek().is_some_and(|ch| !ch.is_whitespace()) {
        cur.bump();
    }
    let name = &buf[name_start..cur.i];
    if name.is_empty() {
        return Ok(Pull::NeedMore);
    }
    if !is_tool_name(name) {
        return Err(Error::Malformed(format!("invalid tool name '{name}'")));
    }
    if cur.eof() {
        return Ok(Pull::NeedMore);
    }
    if !cur.skip_ws() {
        return Err(Error::Malformed(
            "expected whitespace between the tool name and its JSON arguments".into(),
        ));
    }
    if cur.eof() {
        return Ok(Pull::NeedMore);
    }
    let json_start = cur.i;
    match scan_object(&mut cur)? {
        Scan::NeedMore => return Ok(Pull::NeedMore),
        Scan::Done => {}
    }
    let json = &buf[json_start..cur.i];
    cur.skip_ws();
    if cur.eof() {
        return Ok(Pull::NeedMore);
    }
    if !cur.eat('>') {
        return Err(Error::Malformed(
            "expected '>>' after the JSON arguments".into(),
        ));
    }
    if cur.eof() {
        return Ok(Pull::NeedMore);
    }
    if !cur.eat('>') {
        return Err(Error::Malformed(
            "expected '>>' after the JSON arguments".into(),
        ));
    }
    let call = build_call(name, json, tools, index)?;
    Ok(Pull::Call {
        consumed: cur.i,
        call,
    })
}

enum Scan {
    NeedMore,
    Done,
}

fn scan_object(cur: &mut Cur<'_>) -> Result<Scan, Error> {
    if cur.peek() != Some('{') {
        return Err(Error::Malformed("arguments must be a JSON object".into()));
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    loop {
        let Some(ch) = cur.bump() else {
            return Ok(Scan::NeedMore);
        };
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(Scan::Done);
                }
                if depth < 0 {
                    return Err(Error::Malformed("unbalanced '}'".into()));
                }
            }
            _ => {}
        }
    }
}

fn build_call(name: &str, json: &str, tools: &[ToolDef], index: usize) -> Result<ToolCall, Error> {
    let value: Value = serde_json::from_str(json)
        .map_err(|err| Error::Malformed(format!("arguments are not JSON: {err}")))?;
    if !value.is_object() {
        return Err(Error::Malformed("arguments must be a JSON object".into()));
    }
    let tool = lookup(tools, name)?;
    let schema = parameter_schema(tool);
    if let Some(reason) = schema::unsupported_reason(&schema) {
        return Err(Error::UnsupportedSchema(format!("{name}: {reason}")));
    }
    validate(&value, &schema).map_err(|reason| Error::InvalidArguments {
        tool: name.to_string(),
        reason,
    })?;
    let arguments = serde_json::to_string(&value)
        .map_err(|err| Error::Malformed(format!("failed to serialize arguments: {err}")))?;
    Ok(ToolCall::new(index, name.to_string(), arguments))
}

fn lookup<'a>(tools: &'a [ToolDef], name: &str) -> Result<&'a ToolDef, Error> {
    let mut found = None;
    for tool in tools {
        if tool.function.name == name {
            if found.is_some() {
                return Err(Error::UnsupportedSchema(format!(
                    "duplicate tool name '{name}'"
                )));
            }
            found = Some(tool);
        }
    }
    found.ok_or_else(|| Error::UnknownTool(name.to_string()))
}

fn parameter_schema(tool: &ToolDef) -> Value {
    tool.function.parameters.clone().unwrap_or_else(|| {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    })
}

/// Decode every compact call in `text`.
///
/// Ordinary text is ignored. A marker that starts a call (`<<call` plus
/// whitespace) must be a complete, schema-valid call. Unknown tools, missing
/// fields, bad enums, and bad JSON are errors. Nothing is repaired.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    let mut decoder = StreamDecoder::new(tools);
    let mut items = decoder.push(text)?;
    items.extend(decoder.finish()?);
    let mut calls = Vec::new();
    for item in items {
        if let StreamItem::Call(call) = item {
            calls.push(call);
        }
    }
    Ok(calls)
}

/// Render one call in the compact syntax. `arguments` must be a JSON object.
pub fn render_call(name: &str, arguments: &Value) -> Result<String, Error> {
    if !is_tool_name(name) {
        return Err(Error::Malformed(format!("invalid tool name '{name}'")));
    }
    if !arguments.is_object() {
        return Err(Error::Malformed("arguments must be a JSON object".into()));
    }
    let args = serde_json::to_string(arguments)
        .map_err(|err| Error::Malformed(format!("failed to serialize arguments: {err}")))?;
    Ok(format!("<<call {name} {args}>>"))
}
