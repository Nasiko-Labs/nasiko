//! `<<call name {json}>>` decoder. The closing `>>` is recognized only after the
//! JSON value ends, using string state and brace depth.

use crate::schema::{self, check_arguments};
use crate::{Error, Result, ToolCall, ToolDef};

const MARKER: [char; 6] = ['<', '<', 'c', 'a', 'l', 'l'];

#[derive(Debug)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buf: String,
    /// Bytes of a UTF-8 character that arrived split across chunks.
    pending: Vec<u8>,
    calls: Vec<ToolCall>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            buf: String::new(),
            pending: Vec::new(),
            calls: Vec::new(),
        }
    }

    /// Append `chunk` (`&str` or raw bytes). Returns calls completed by this chunk.
    /// A call split across chunks stays buffered. A UTF-8 character split across
    /// chunks stays in `pending` until the sequence is complete.
    pub fn push(&mut self, chunk: impl AsRef<[u8]>) -> Result<Vec<ToolCall>> {
        self.pending.extend_from_slice(chunk.as_ref());
        let text = take_utf8(&mut self.pending)?;
        self.buf.push_str(&text);
        let (new_calls, rest) = take_calls(&self.buf, &self.tools)?;
        self.buf = rest;
        self.calls.extend(new_calls.iter().cloned());
        Ok(new_calls)
    }

    /// Every call completed so far.
    /// An opened `<<call` that never closed, or a trailing partial UTF-8 sequence,
    /// is [`Error::InvalidArguments`].
    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        if !self.pending.is_empty() || self.buf.starts_with("<<call") {
            return Err(Error::InvalidArguments);
        }
        Ok(self.calls.clone())
    }
}

/// Complete UTF-8 is returned and removed. A sequence cut off at the end stays in `pending`.
fn take_utf8(pending: &mut Vec<u8>) -> Result<String> {
    match std::str::from_utf8(pending) {
        Ok(text) => {
            let out = text.to_string();
            pending.clear();
            Ok(out)
        }
        Err(err) if err.error_len().is_none() => {
            let n = err.valid_up_to();
            let Some(bytes) = pending.get(..n) else {
                return Err(Error::InvalidArguments);
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Err(Error::InvalidArguments);
            };
            let out = text.to_string();
            pending.drain(..n);
            Ok(out)
        }
        Err(_) => Err(Error::InvalidArguments),
    }
}

pub(crate) fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools);
    decoder.push(text)?;
    decoder.finish()
}

enum Step {
    Call { call: ToolCall, next: usize },
    NeedMore,
}

fn take_calls(text: &str, tools: &[ToolDef]) -> Result<(Vec<ToolCall>, String)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    let mut calls = Vec::new();
    while i < chars.len() {
        if partial_marker(&chars, i) {
            return Ok((calls, owned_suffix(text, chars[i].0)?));
        }
        if marker_at(&chars, i) {
            match parse_call(text, &chars, i, tools)? {
                Step::NeedMore => return Ok((calls, owned_suffix(text, chars[i].0)?)),
                Step::Call { call, next } => {
                    if next <= i {
                        return Err(Error::InvalidArguments);
                    }
                    calls.push(call);
                    i = next;
                }
            }
        } else {
            i += 1;
        }
    }
    Ok((calls, String::new()))
}

fn partial_marker(chars: &[(usize, char)], i: usize) -> bool {
    let rest = chars.len() - i;
    rest > 0
        && rest < MARKER.len()
        && chars[i..]
            .iter()
            .zip(MARKER)
            .all(|(item, marker)| item.1 == marker)
}

fn marker_at(chars: &[(usize, char)], i: usize) -> bool {
    if i + MARKER.len() > chars.len() {
        return false;
    }
    let keyword = chars[i..i + MARKER.len()]
        .iter()
        .zip(MARKER)
        .all(|(item, marker)| item.1 == marker);
    keyword
        && chars
            .get(i + MARKER.len())
            .is_none_or(|item| item.1.is_whitespace())
}

fn parse_call(text: &str, chars: &[(usize, char)], i: usize, tools: &[ToolDef]) -> Result<Step> {
    let mut j = i + MARKER.len();
    if j >= chars.len() {
        return Ok(Step::NeedMore);
    }
    if !chars[j].1.is_whitespace() {
        return Err(Error::InvalidArguments);
    }
    j = skip_ws(chars, j);
    if j >= chars.len() {
        return Ok(Step::NeedMore);
    }
    let (name, mut j) = match read_name(text, chars, j)? {
        None => return Ok(Step::NeedMore),
        Some(pair) => pair,
    };
    j = skip_ws(chars, j);
    if j >= chars.len() {
        return Ok(Step::NeedMore);
    }
    if chars[j].1 != '{' {
        return Err(Error::InvalidArguments);
    }
    let Some(json_end) = scan_json(chars, j)? else {
        return Ok(Step::NeedMore);
    };
    let mut k = skip_ws(chars, json_end);
    if k >= chars.len() {
        return Ok(Step::NeedMore);
    }
    if chars[k].1 != '>' {
        return Err(Error::InvalidArguments);
    }
    k += 1;
    if k >= chars.len() {
        return Ok(Step::NeedMore);
    }
    if chars[k].1 != '>' {
        return Err(Error::InvalidArguments);
    }
    k += 1;
    let raw = slice_bytes(text, chars[j].0, json_byte(chars, json_end, text.len()))?;
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|_| Error::InvalidArguments)?;
    let Some(tool) = tools.iter().find(|tool| tool.name == name) else {
        return Err(Error::UnknownTool);
    };
    check_arguments(&tool.parameters, &value)?;
    Ok(Step::Call {
        call: ToolCall {
            name,
            arguments: raw.to_string(),
        },
        next: k,
    })
}

fn json_byte(chars: &[(usize, char)], index: usize, len: usize) -> usize {
    chars.get(index).map(|item| item.0).unwrap_or(len)
}

fn slice_bytes(text: &str, start: usize, end: usize) -> Result<&str> {
    text.get(start..end).ok_or(Error::InvalidArguments)
}

fn skip_ws(chars: &[(usize, char)], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|item| item.1.is_whitespace()) {
        i += 1;
    }
    i
}

fn read_name(text: &str, chars: &[(usize, char)], i: usize) -> Result<Option<(String, usize)>> {
    if chars.get(i).is_some_and(|item| item.1 == '"') {
        return read_quoted_name(text, chars, i);
    }
    if !chars
        .get(i)
        .is_some_and(|item| schema::is_ident_start(item.1))
    {
        return Err(Error::InvalidArguments);
    }
    let mut j = i;
    let mut name = String::new();
    while chars
        .get(j)
        .is_some_and(|item| schema::is_ident_cont(item.1))
    {
        name.push(chars[j].1);
        j += 1;
    }
    if j == chars.len() {
        return Ok(None);
    }
    Ok(Some((name, j)))
}

fn read_quoted_name(
    text: &str,
    chars: &[(usize, char)],
    i: usize,
) -> Result<Option<(String, usize)>> {
    let mut j = i + 1;
    let mut escape = false;
    while j < chars.len() {
        let ch = chars[j].1;
        j += 1;
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            let raw = slice_bytes(
                text,
                chars[i].0,
                chars.get(j).map(|item| item.0).unwrap_or(text.len()),
            )?;
            let name: String = serde_json::from_str(raw).map_err(|_| Error::InvalidArguments)?;
            return Ok(Some((name, j)));
        }
    }
    Ok(None)
}

/// `Ok(None)` means the value is cut off. The end index is one past the closing brace.
fn scan_json(chars: &[(usize, char)], start: usize) -> Result<Option<usize>> {
    if chars.get(start).is_none_or(|item| item.1 != '{') {
        return Err(Error::InvalidArguments);
    }
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut j = start;
    while j < chars.len() {
        let ch = chars[j].1;
        j += 1;
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
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(Some(j));
                }
                if depth < 0 {
                    return Err(Error::InvalidArguments);
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

fn owned_suffix(text: &str, byte: usize) -> Result<String> {
    text.get(byte..)
        .map(str::to_string)
        .ok_or(Error::InvalidArguments)
}
