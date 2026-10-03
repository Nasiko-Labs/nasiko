//! Incremental decoder. One-shot decoding is this decoder fed a single chunk, so streaming and
//! non-streaming share one code path.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::call::{MARKER, Scan, call_name, parse_args, scan_call};
use crate::error::{Error, Result};
use crate::ty::{Obj, params_from_schema};
use crate::types::{Event, ToolCall, ToolDef};
use crate::validate::check_args;

/// Decodes model output chunk by chunk.
///
/// Text is released as soon as it cannot be the start of a call marker; a possible marker
/// prefix (`<`, `<<`, `<<ca`, …) is held back until the next chunk decides it. Once a call fails,
/// the decoder stays failed and returns the same error.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: BTreeMap<String, Obj>,
    buf: String,
    in_call: bool,
    failed: Option<Error>,
}

impl StreamDecoder {
    /// Build a decoder that validates calls against `tools`.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let mut map = BTreeMap::new();
        for t in tools {
            let params =
                params_from_schema(t.parameters.as_ref()).map_err(|reason| Error::Unsupported {
                    tool: t.name.clone(),
                    reason,
                })?;
            if map.insert(t.name.clone(), params).is_some() {
                return Err(Error::Unsupported {
                    tool: t.name.clone(),
                    reason: "duplicate tool name".into(),
                });
            }
        }
        Ok(Self {
            tools: map,
            buf: String::new(),
            in_call: false,
            failed: None,
        })
    }

    /// Feed the next chunk; returns whatever it completes.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<Event>> {
        self.run(chunk, false)
    }

    /// End of output. Held-back text is released; an unclosed call is an error.
    pub fn finish(mut self) -> Result<Vec<Event>> {
        self.run("", true)
    }

    fn run(&mut self, chunk: &str, eof: bool) -> Result<Vec<Event>> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        self.buf.push_str(chunk);
        let mut events = Vec::new();
        match self.drain(&mut events, eof) {
            Ok(()) => Ok(events),
            Err(e) => {
                self.failed = Some(e.clone());
                Err(e)
            }
        }
    }

    fn drain(&mut self, events: &mut Vec<Event>, eof: bool) -> Result<()> {
        loop {
            if self.in_call {
                if let Some(name) = call_name(&self.buf)
                    && !self.tools.contains_key(name)
                {
                    return Err(Error::UnknownTool(name.to_string()));
                }
                let (len, call) = match scan_call(&self.buf) {
                    Scan::Incomplete if eof => {
                        return Err(Error::Malformed("output ended inside a call".into()));
                    }
                    Scan::Incomplete => return Ok(()),
                    Scan::Malformed(reason) => return Err(Error::Malformed(reason)),
                    Scan::Done { len, name, args } => (len, self.validate(name, args)?),
                };
                events.push(Event::Call(call));
                self.buf.drain(..len);
                self.in_call = false;
                continue;
            }

            let Some(i) = self.buf.find(MARKER) else {
                let hold = if eof {
                    0
                } else {
                    (1..MARKER.len())
                        .rev()
                        .find(|&k| self.buf.ends_with(&MARKER[..k]))
                        .unwrap_or(0)
                };
                self.release(events, self.buf.len() - hold);
                return Ok(());
            };
            let after = i + MARKER.len();
            match self.buf.as_bytes().get(after) {
                // `<<call` at the very end: wait for the next byte, or it was just text.
                None if !eof => {
                    self.release(events, i);
                    return Ok(());
                }
                None => {
                    self.release(events, self.buf.len());
                    return Ok(());
                }
                Some(c) if c.is_ascii_whitespace() => {
                    self.release(events, i);
                    self.in_call = true;
                }
                // `<<calls`, `<<call>` …: not a marker.
                Some(_) => self.release(events, after),
            }
        }
    }

    /// Emit the first `n` buffered bytes as text.
    fn release(&mut self, events: &mut Vec<Event>, n: usize) {
        if n == 0 {
            return;
        }
        let text: String = self.buf.drain(..n).collect();
        match events.last_mut() {
            Some(Event::Text(t)) => t.push_str(&text),
            _ => events.push(Event::Text(text)),
        }
    }

    fn validate(&self, name: &str, args: &str) -> Result<ToolCall> {
        let params = self
            .tools
            .get(name)
            .ok_or_else(|| Error::UnknownTool(name.to_string()))?;
        let invalid = |reason| Error::InvalidArguments {
            tool: name.to_string(),
            reason,
        };
        let arguments = parse_args(args).map_err(invalid)?;
        check_args(params, &Value::Object(arguments.clone())).map_err(invalid)?;
        Ok(ToolCall {
            name: name.to_string(),
            arguments,
        })
    }
}
