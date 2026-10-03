use serde_json::Value;

use crate::schema::validate_arguments;
use crate::{CompactError, Result, ToolCall, ToolDef};

const START: &str = "<<call";
const MAX_BUFFER: usize = 1_048_576;

/// Render a call in the only model-facing call grammar. JSON handles escaping.
pub fn render_call(call: &ToolCall) -> String {
    format!("<<call {} {}>>", call.name, call.arguments)
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}

/// Remove validated compact markers while retaining surrounding assistant prose.
pub fn strip_calls(text: &str, tools: &[ToolDef]) -> Result<String> {
    let mut out = String::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(START) {
        let start = cursor + offset;
        out.push_str(&text[cursor..start]);
        let (end, _) = parse_candidate(text, start, tools)?
            .ok_or_else(|| CompactError::MalformedCall("truncated call".into()))?;
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    Ok(out.trim().to_string())
}

/// Incrementally scans completed markers, including markers split at any chunk edge.
/// A recognized malformed call fails closed. No call is emitted before validation.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    cursor: usize,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let mut names = std::collections::HashSet::new();
        for tool in tools {
            if !names.insert(&tool.name) {
                return Err(CompactError::InvalidDefinition(
                    "duplicate tool name".into(),
                ));
            }
        }
        Ok(Self {
            tools: tools.to_vec(),
            buffer: String::new(),
            cursor: 0,
        })
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        self.buffer.push_str(chunk);
        let mut calls = Vec::new();
        loop {
            let Some(offset) = self.buffer[self.cursor..].find(START) else {
                // Retain the tail because it may be a prefix completed by the next chunk.
                self.cursor = self.buffer.len().saturating_sub(START.len() - 1);
                while !self.buffer.is_char_boundary(self.cursor) {
                    self.cursor -= 1;
                }
                break;
            };
            let start = self.cursor + offset;
            match parse_candidate(&self.buffer, start, &self.tools)? {
                Some((end, call)) => {
                    calls.push(call);
                    self.cursor = end;
                }
                None => {
                    self.cursor = start;
                    break;
                }
            }
        }
        if self.cursor > 64 * 1024 {
            self.buffer.drain(..self.cursor);
            self.cursor = 0;
        }
        if self.buffer.len() > MAX_BUFFER {
            return Err(CompactError::MalformedCall(
                "call exceeds decoder limit".into(),
            ));
        }
        Ok(calls)
    }

    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        let remaining = &self.buffer[self.cursor..];
        if remaining.contains(START) || (2..START.len()).any(|n| remaining.ends_with(&START[..n])) {
            return Err(CompactError::MalformedCall("truncated call".into()));
        }
        Ok(Vec::new())
    }
}

/// A complete candidate returns its end byte offset and validated call.
/// `None` means more bytes are needed; malformed complete candidates return an error.
fn parse_candidate(
    text: &str,
    start: usize,
    tools: &[ToolDef],
) -> Result<Option<(usize, ToolCall)>> {
    let bytes = text.as_bytes();
    let mut i = start + START.len();
    if i >= bytes.len() {
        return Ok(None);
    }
    if bytes[i] != b' ' {
        return Err(CompactError::MalformedCall(
            "expected space after call marker".into(),
        ));
    }
    i += 1;
    if i >= bytes.len() {
        return Ok(None);
    }
    let name_start = i;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
    {
        i += 1;
    }
    if i == bytes.len() {
        return Ok(None);
    }
    if i == name_start || bytes[i] != b' ' {
        return Err(CompactError::MalformedCall(
            "expected tool name and space".into(),
        ));
    }
    let name = &text[name_start..i];
    i += 1;
    while i < bytes.len() && bytes[i] == b' ' {
        i += 1;
    }
    if i >= bytes.len() {
        return Ok(None);
    }
    if bytes[i] != b'{' {
        return Err(CompactError::MalformedCall(
            "arguments must begin with an object".into(),
        ));
    }
    let json_start = i;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut json_end = None;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        json_end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    let Some(json_end) = json_end else {
        return Ok(None);
    };
    i = json_end;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || (bytes[i] == b'>' && i + 1 == bytes.len()) {
        return Ok(None);
    }
    if !text[i..].starts_with(">>") {
        return Err(CompactError::MalformedCall(
            "expected >> after arguments".into(),
        ));
    }
    let arguments: Value = serde_json::from_str(&text[json_start..json_end])
        .map_err(|e| CompactError::InvalidArguments(e.to_string()))?;
    let tool = tools
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| CompactError::UnknownTool(name.to_string()))?;
    validate_arguments(tool.parameters.as_ref(), &arguments)?;
    Ok(Some((
        i + 2,
        ToolCall {
            name: name.to_string(),
            arguments,
        },
    )))
}
