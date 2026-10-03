//! Decoder — parses model output in the compact grammar back into standard tool calls.
//!
//! # Grammar recap
//!
//! ```text
//! call ::= "<<call" SP name SP json_object ">>"
//! ```
//!
//! `>>` terminates the call only when it is **not inside a JSON string literal**.
//! The decoder tracks a small state machine to handle JSON string escaping and
//! markers that are split across stream chunks.
//!
//! # Validation (fail-closed)
//!
//! After extracting `(name, json_args)` the decoder validates:
//! 1. `name` exists in the known-tools registry → `unknown_tool` on failure.
//! 2. The JSON parses as an object → `invalid_arguments` on failure.
//! 3. All required fields are present and have the right JSON type → `invalid_arguments`.
//! 4. Enum fields carry a listed value → `invalid_arguments`.
//!
//! On any validation failure the entire call is an error; no partial or guessed call is
//! returned.

use serde_json::Value;

use crate::types::{FunctionCall, FunctionDef, ToolCall, ToolDef};

// ─────────────────────────────────────────────────────────────────────────────
// Public result type
// ─────────────────────────────────────────────────────────────────────────────

/// The outcome of decoding a chunk of model output.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodeResult {
    /// Successfully validated tool calls found in the text.
    pub calls: Vec<ToolCall>,
    /// First error encountered (if any). The crate is fail-closed: when an error
    /// is set, `calls` contains only the calls that were decoded **before** the
    /// error occurred.
    pub error: Option<DecodeError>,
}

/// Decode errors. Named to match the eval harness expectations.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown_tool")]
    UnknownTool,
    #[error("invalid_arguments")]
    InvalidArguments,
    #[error("malformed: {0}")]
    Malformed(String),
}

impl DecodeError {
    /// The stable wire string used in the eval JSONL output.
    pub fn as_wire(&self) -> &'static str {
        match self {
            DecodeError::UnknownTool => "unknown_tool",
            DecodeError::InvalidArguments => "invalid_arguments",
            DecodeError::Malformed(_) => "invalid_arguments",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming decoder
// ─────────────────────────────────────────────────────────────────────────────

/// Incremental decoder that handles model output arriving as a stream of string chunks.
///
/// Feed chunks with [`StreamDecoder::push`]; call [`StreamDecoder::finish`] after the
/// last chunk to flush any trailing buffered text and retrieve the final result.
///
/// Handles:
/// - Call markers split across chunk boundaries.
/// - `>>` inside JSON string values (not treated as terminators).
/// - Multiple calls in one output.
/// - Plain text before, after, or between calls (ignored).
#[derive(Debug)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    /// Accumulates all text seen so far.
    buf: String,
    calls: Vec<ToolCall>,
    error: Option<DecodeError>,
}

impl StreamDecoder {
    /// Create a new decoder that validates calls against `tools`.
    pub fn new(tools: Vec<ToolDef>) -> Self {
        StreamDecoder {
            tools,
            buf: String::new(),
            calls: Vec::new(),
            error: None,
        }
    }

    /// Push the next chunk of model output.
    pub fn push(&mut self, chunk: &str) {
        if self.error.is_some() {
            return; // fail-closed: stop after first error
        }
        self.buf.push_str(chunk);
        self.drain();
    }

    /// Finish streaming and return the final result.
    pub fn finish(self) -> DecodeResult {
        // No more chunks; anything left in buf is trailing text (no call).
        DecodeResult {
            calls: self.calls,
            error: self.error,
        }
    }

    // ── internal ─────────────────────────────────────────────────────────────

    /// Try to extract and process all complete <<call ... >> blocks from `self.buf`.
    fn drain(&mut self) {
        loop {
            // Find the start marker.
            let Some(start) = self.buf.find("<<call ") else {
                // No open marker yet; keep everything (it might be a partial "<<cal").
                // But trim the part that can't be a marker start.
                self.trim_non_marker_prefix();
                return;
            };

            // Find the end marker, respecting JSON string context.
            let after_start = start + "<<call ".len();
            let rest = &self.buf[after_start..];
            match find_closing(rest) {
                FindResult::Found(end_in_rest) => {
                    // Extract the payload as an owned String so that the immutable
                    // borrow on `self.buf` ends before we call `self.process_call`
                    // (which needs `&mut self`).
                    let payload = rest[..end_in_rest].to_owned();
                    let consumed = after_start + end_in_rest + ">>".len();
                    if let Err(e) = self.process_call(&payload) {
                        self.error = Some(e);
                        return;
                    }
                    // Consume up to and including the ">>"
                    self.buf.drain(..consumed);
                }
                FindResult::NeedMore => {
                    // The closing >> hasn't arrived yet; keep the buffer as-is.
                    // Remove any leading text before the <<call so future pushes
                    // don't search through it again.
                    if start > 0 {
                        self.buf.drain(..start);
                    }
                    return;
                }
            }
        }
    }

    /// Remove a prefix of `self.buf` that cannot be the start of `<<call `.
    fn trim_non_marker_prefix(&mut self) {
        // We want to keep any suffix that could be a partial "<<call" (up to 7 chars).
        const MARKER: &str = "<<call ";
        let keep = MARKER.len() - 1; // 6 bytes — the longest partial to keep
        if self.buf.len() > keep {
            // Keep only the last `keep` bytes at most; they might be the start of <<call.
            let trim_to = self.buf.len() - keep;
            // But only trim up to the point where there's no '<' that could start <<call.
            // Find the last '<' in the trimmable prefix.
            let trimmable = &self.buf[..trim_to];
            if let Some(lt) = trimmable.rfind('<') {
                if lt > 0 {
                    self.buf.drain(..lt);
                }
            } else {
                self.buf.drain(..trim_to);
            }
        }
    }

    fn process_call(&mut self, payload: &str) -> Result<(), DecodeError> {
        // payload = "name {json}"
        let payload = payload.trim();
        let (name, json_part) = split_name_and_args(payload)?;
        let tool = self
            .tools
            .iter()
            .find(|t| t.function.name == name)
            .ok_or(DecodeError::UnknownTool)?;
        let args: Value = serde_json::from_str(json_part)
            .map_err(|_| DecodeError::InvalidArguments)?;
        validate_args(&args, &tool.function)?;
        let arguments = serde_json::to_string(&args).map_err(|_| DecodeError::InvalidArguments)?;
        self.calls.push(ToolCall {
            kind: "function".into(),
            function: FunctionCall {
                name: name.to_string(),
                arguments,
            },
        });
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Stateless entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Decode all `<<call ...>>` blocks from `text` and validate them against `tools`.
///
/// Equivalent to feeding the entire text as a single chunk and calling `finish`.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> DecodeResult {
    let mut dec = StreamDecoder::new(tools.to_vec());
    dec.push(text);
    dec.finish()
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

enum FindResult {
    Found(usize), // byte offset of ">>" within the searched slice
    NeedMore,
}

/// Find the first `>>` that is outside a JSON string, within `s` (which starts right
/// after `"<<call "`).
///
/// Tracks JSON string state: inside a string `>>` is literal. The state machine is
/// minimal — it only needs to track whether we're in a string, handling `\"` escapes.
fn find_closing(s: &str) -> FindResult {
    let bytes = s.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        if in_string {
            match bytes[i] {
                b'\\' => {
                    i += 2; // skip escaped char
                    continue;
                }
                b'"' => {
                    in_string = false;
                }
                _ => {}
            }
        } else {
            match bytes[i] {
                b'"' => {
                    in_string = true;
                }
                b'>' if bytes.get(i + 1) == Some(&b'>') => {
                    return FindResult::Found(i);
                }
                _ => {}
            }
        }
        i += 1;
    }
    FindResult::NeedMore
}

/// Split `"tool_name {...json...}"` into `(name, json_str)`.
fn split_name_and_args(payload: &str) -> Result<(&str, &str), DecodeError> {
    // Name is the first whitespace-delimited token.
    let Some(sp) = payload.find(|c: char| c.is_whitespace()) else {
        return Err(DecodeError::Malformed(
            "no arguments after tool name".into(),
        ));
    };
    let name = &payload[..sp];
    let rest = payload[sp..].trim_start();
    Ok((name, rest))
}

// ─────────────────────────────────────────────────────────────────────────────
// Schema validation
// ─────────────────────────────────────────────────────────────────────────────

fn validate_args(args: &Value, func: &FunctionDef) -> Result<(), DecodeError> {
    let obj = args.as_object().ok_or(DecodeError::InvalidArguments)?;

    let Some(params) = &func.parameters else {
        // No schema → accept anything.
        return Ok(());
    };

    let required: Vec<String> = params
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    // Check required fields are present.
    for req in &required {
        if !obj.contains_key(req.as_str()) {
            return Err(DecodeError::InvalidArguments);
        }
    }

    let props = params
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();

    // Validate each provided field against its schema.
    for (key, val) in obj {
        if let Some(schema) = props.get(key) {
            validate_field_value(val, schema)?;
        }
        // Extra fields not in schema are allowed (permissive by default — the model
        // may emit benign extras; fail-closed only on type/enum violations).
    }

    Ok(())
}

fn validate_field_value(val: &Value, schema: &Value) -> Result<(), DecodeError> {
    // Enum check takes priority.
    if let Some(enums) = schema.get("enum").and_then(|e| e.as_array()) {
        if !enums.contains(val) {
            return Err(DecodeError::InvalidArguments);
        }
        return Ok(());
    }

    // Type check.
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => {
            if !val.is_string() {
                return Err(DecodeError::InvalidArguments);
            }
        }
        Some("integer") => {
            if !val.is_i64() && !val.is_u64() {
                return Err(DecodeError::InvalidArguments);
            }
        }
        Some("number") => {
            if !val.is_number() {
                return Err(DecodeError::InvalidArguments);
            }
        }
        Some("boolean") => {
            if !val.is_boolean() {
                return Err(DecodeError::InvalidArguments);
            }
        }
        Some("array") => {
            let arr = val.as_array().ok_or(DecodeError::InvalidArguments)?;
            if let Some(item_schema) = schema.get("items") {
                for item in arr {
                    validate_field_value(item, item_schema)?;
                }
            }
        }
        Some("object") => {
            if !val.is_object() {
                return Err(DecodeError::InvalidArguments);
            }
        }
        _ => {} // unknown type → permissive
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDef, ToolDef};
    use serde_json::json;

    fn cal_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "duration_min": {"type": "integer"},
                        "attendees": {"type": "array", "items": {"type": "string"}},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
        }
    }

    fn email_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: Some("Send email.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}},
                        "subject": {"type": "string"},
                        "body": {"type": "string"},
                        "cc": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        }
    }

    // ── dc-001: valid single call ─────────────────────────────────────────────
    #[test]
    fn dc001_valid_single_call() {
        let text = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let result = decode_calls(text, &[cal_tool()]);
        assert!(result.error.is_none());
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.calls[0].function.name, "create_calendar_event");
    }

    // ── dc-002: marker split across stream chunks ─────────────────────────────
    #[test]
    fn dc002_split_chunks() {
        let tools = vec![cal_tool()];
        let mut dec = StreamDecoder::new(tools);
        dec.push("<<ca");
        dec.push(r#"ll create_calendar_event {"title":"Ret"#);
        dec.push(r#"ro","start":"2026-10-04T10:00:00+05:30"}>"#);
        dec.push(">");
        let result = dec.finish();
        assert!(result.error.is_none(), "error: {:?}", result.error);
        assert_eq!(result.calls.len(), 1);
        let args: Value = serde_json::from_str(&result.calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "Retro");
    }

    // ── dc-003: ">>" inside a string argument ─────────────────────────────────
    #[test]
    fn dc003_gt_inside_string() {
        let tools = vec![email_tool()];
        let text = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let result = decode_calls(text, &tools);
        assert!(result.error.is_none(), "error: {:?}", result.error);
        assert_eq!(result.calls.len(), 1);
        let args: Value =
            serde_json::from_str(&result.calls[0].function.arguments).unwrap();
        assert_eq!(args["subject"], "a >> b");
    }

    // ── dc-004: unknown tool ──────────────────────────────────────────────────
    #[test]
    fn dc004_unknown_tool() {
        let result = decode_calls(
            r#"<<call delete_everything {}>>"#,
            &[cal_tool()],
        );
        assert_eq!(result.error, Some(DecodeError::UnknownTool));
    }

    // ── dc-005: missing required field + bad enum ─────────────────────────────
    #[test]
    fn dc005_missing_required_and_bad_enum() {
        let text =
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
        let result = decode_calls(text, &[cal_tool()]);
        assert_eq!(result.error, Some(DecodeError::InvalidArguments));
    }

    // ── plain answer with no call ─────────────────────────────────────────────
    #[test]
    fn plain_answer_no_calls() {
        let result = decode_calls("What's the weather? I don't have a weather tool.", &[cal_tool()]);
        assert!(result.calls.is_empty());
        assert!(result.error.is_none());
    }

    // ── multiple calls in one output ──────────────────────────────────────────
    #[test]
    fn multiple_calls() {
        let tools = vec![cal_tool(), email_tool()];
        let text = r#"<<call send_email {"to":["a@b.com"],"subject":"hi","body":"hello"}>><<call create_calendar_event {"title":"Sync","start":"2026-10-05T10:00:00+05:30"}>>"#;
        let result = decode_calls(text, &tools);
        assert!(result.error.is_none());
        assert_eq!(result.calls.len(), 2);
    }

    // ── text surrounding calls ────────────────────────────────────────────────
    #[test]
    fn text_around_calls() {
        let text = r#"Sure, I'll book it. <<call create_calendar_event {"title":"Standup","start":"2026-10-05T09:00:00+05:30"}>> Done!"#;
        let result = decode_calls(text, &[cal_tool()]);
        assert!(result.error.is_none());
        assert_eq!(result.calls.len(), 1);
    }

    // ── malformed JSON args ───────────────────────────────────────────────────
    #[test]
    fn malformed_json() {
        let text = r#"<<call create_calendar_event {not valid json}>>"#;
        let result = decode_calls(text, &[cal_tool()]);
        assert!(result.error.is_some());
    }
}
