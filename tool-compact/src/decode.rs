//! Model text -> validated calls. Grammar: see `GRAMMAR.md`.
//!
//! ```text
//! call  = "<<call " name WS+ json-object WS* ">>"
//! ```
//! Anything outside a call is plain text and ignored. One state machine serves both the
//! one-shot and the streaming entry points, so they cannot disagree.

use std::collections::HashMap;

use serde_json::Value;

use crate::encode::check_tool;
use crate::validate::validate_args;
use crate::{Error, Result, ToolCall, ToolDef, is_name_char};

const OPEN: &str = "<<call ";
const CLOSE: &str = ">>";

/// Decodes every call in `text`. All-or-nothing: one bad call fails the whole reply.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish()?);
    Ok(calls)
}

/// A decoded reply: the model's plain text (calls removed) and its calls.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// Like [`decode_calls`], but also returns the text around the calls (trimmed), so a caller can
/// show the user what the model said without the call markers.
pub fn decode_reply(text: &str, tools: &[ToolDef]) -> Result<Reply> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut calls = decoder.push(text)?;
    calls.extend(decoder.finish_in_place()?);
    Ok(Reply {
        text: decoder.text.trim().to_string(),
        calls,
    })
}

/// Incremental decoder. Feed chunks with [`push`](Self::push); each call is returned once, as
/// soon as its closing `>>` arrives and it validates. Call [`finish`](Self::finish) at end of
/// stream: an unterminated call is an error. After any error the decoder stays failed.
pub struct StreamDecoder {
    tools: HashMap<String, Option<Value>>,
    buf: String,
    /// Plain text seen so far (everything outside calls).
    text: String,
    failed: Option<Error>,
}

impl StreamDecoder {
    /// `Err(Error::Bypass)` if a tool uses a schema feature this format cannot verify.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        tools.iter().try_for_each(check_tool)?;
        let tools = tools
            .iter()
            .map(|t| (t.name.clone(), t.parameters.clone()))
            .collect();
        Ok(Self {
            tools,
            buf: String::new(),
            text: String::new(),
            failed: None,
        })
    }

    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        self.drain(false)
    }

    pub fn finish(mut self) -> Result<Vec<ToolCall>> {
        self.finish_in_place()
    }

    fn finish_in_place(&mut self) -> Result<Vec<ToolCall>> {
        if let Some(e) = self.failed.take() {
            return Err(e);
        }
        self.drain(true)
    }

    fn drain(&mut self, last: bool) -> Result<Vec<ToolCall>> {
        let result = self.drain_inner(last);
        if let Err(e) = &result {
            self.failed = Some(e.clone());
        }
        result
    }

    fn drain_inner(&mut self, last: bool) -> Result<Vec<ToolCall>> {
        let mut out = vec![];
        loop {
            let Some(start) = self.buf.find(OPEN) else {
                // Plain text. Keep only a tail that could still grow into the marker.
                let keep = if last {
                    0
                } else {
                    partial_marker_len(&self.buf)
                };
                let cut = self.buf.len() - keep;
                self.text.push_str(&self.buf[..cut]);
                self.buf.drain(..cut);
                return Ok(out);
            };
            self.text.push_str(&self.buf[..start]);
            self.buf.drain(..start);
            match self.parse_call(last)? {
                Some((call, consumed)) => {
                    out.push(call);
                    self.buf.drain(..consumed);
                }
                None => return Ok(out), // incomplete: wait for more input
            }
        }
    }

    /// `buf` starts with the marker. `Ok(None)` = need more input.
    fn parse_call(&self, last: bool) -> Result<Option<(ToolCall, usize)>> {
        let incomplete = |what: &str| {
            if last {
                Err(Error::Malformed(format!("unterminated call: {what}")))
            } else {
                Ok(None)
            }
        };
        let rest = &self.buf[OPEN.len()..];
        let Some(name_len) = rest.find(|c: char| !is_name_char(c)) else {
            return incomplete("tool name");
        };
        let name = &rest[..name_len];
        if name.is_empty() || !rest[name_len..].starts_with(char::is_whitespace) {
            return Err(Error::Malformed("expected `<<call NAME {json}>>`".into()));
        }
        let schema = self
            .tools
            .get(name)
            .ok_or_else(|| Error::UnknownTool(name.to_string()))?;

        let after_name = &rest[name_len..];
        let body = after_name.trim_start();
        let json_start = OPEN.len() + name_len + (after_name.len() - body.len());
        if body.is_empty() {
            return incomplete("arguments");
        }
        if !body.starts_with('{') {
            return Err(Error::Malformed("arguments must be a JSON object".into()));
        }
        let mut values = serde_json::Deserializer::from_str(body).into_iter::<Value>();
        let args = match values.next() {
            Some(Ok(v)) => v,
            Some(Err(e)) if !e.is_eof() => return Err(Error::Malformed(format!("bad JSON: {e}"))),
            _ => return incomplete("arguments"),
        };
        let json_len = values.byte_offset();

        let tail = &body[json_len..];
        let closing = tail.trim_start();
        if closing.len() < CLOSE.len() && CLOSE.starts_with(closing) {
            return incomplete("closing `>>`");
        }
        if !closing.starts_with(CLOSE) {
            return Err(Error::Malformed("expected `>>` after the arguments".into()));
        }
        validate_args(name, schema, &args)?;

        let call = ToolCall {
            name: name.to_string(),
            arguments: body[..json_len].to_string(),
        };
        let consumed = json_start + json_len + (tail.len() - closing.len()) + CLOSE.len();
        Ok(Some((call, consumed)))
    }
}

/// Length of the longest suffix of `s` that is a proper prefix of the marker.
fn partial_marker_len(s: &str) -> usize {
    (1..OPEN.len())
        .rev()
        .find(|&k| s.ends_with(&OPEN[..k]))
        .unwrap_or(0)
}
