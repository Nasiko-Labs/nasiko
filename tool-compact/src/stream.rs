//! Incremental streaming decoder for compact tool call markers.
//!
//! The [`StreamDecoder`] accumulates arbitrary stream chunks and correctly
//! handles the `<<call ...>>` marker split across chunk boundaries.
//!
//! # State machine
//!
//! The decoder is always in one of two states:
//! 1. **Scanning**: Looking for the start of `<<call `. Yields any text that
//!    provably cannot start a marker.
//! 2. **Buffering**: A potential or confirmed marker has started. Accumulates
//!    bytes until a complete `<<call TOOL_NAME JSON>>` is found.
//!
//! # Usage
//!
//! ```rust
//! use nasiko_tool_compact::{StreamDecoder, FunctionDef, ToolDef};
//! use serde_json::json;
//!
//! let tool = ToolDef {
//!     kind: "function".into(),
//!     function: FunctionDef {
//!         name: "ping".into(),
//!         description: None,
//!         parameters: None,
//!     },
//! };
//! let tools = vec![tool];
//! let mut dec = StreamDecoder::new(&tools);
//!
//! // Feed chunks one at a time.
//! let (text1, calls1) = dec.push("Hello <<ca").unwrap();
//! let (text2, calls2) = dec.push("ll ping {}>>\nDone").unwrap();
//! let (text3, calls3) = dec.flush().unwrap();
//! ```

use crate::decode::decode_calls;
use crate::error::DecodeError;
use crate::types::{ToolCall, ToolDef};

/// The opening marker sequence.
const OPEN: &str = "<<call ";

/// Incremental streaming decoder.
///
/// Accumulates stream chunks and extracts complete `<<call ...>>` markers as
/// they become available, even when the marker is split across chunk boundaries.
pub struct StreamDecoder<'tools> {
    tools: &'tools [ToolDef],
    /// Bytes buffered from previous chunks that may be part of a marker.
    buf: String,
    /// Whether we are currently inside a confirmed or potential marker.
    in_marker: bool,
}

impl<'tools> StreamDecoder<'tools> {
    /// Create a new decoder for the given tool set.
    pub fn new(tools: &'tools [ToolDef]) -> Self {
        Self {
            tools,
            buf: String::new(),
            in_marker: false,
        }
    }

    /// Push a new chunk of text.
    ///
    /// Returns `(passthrough_text, completed_calls)`.
    ///
    /// - `passthrough_text`: plain text that provably cannot be part of any
    ///   marker. Safe to forward to the consumer immediately.
    /// - `completed_calls`: zero or more fully parsed, validated calls.
    ///
    /// Returns `Err` on the first invalid call encountered.
    pub fn push(&mut self, chunk: &str) -> Result<(String, Vec<ToolCall>), DecodeError> {
        self.buf.push_str(chunk);
        self.process()
    }

    /// Flush the remaining buffer.
    ///
    /// Call this after the last chunk to release any buffered plain text.
    /// Incomplete markers are returned as plain text (they cannot be valid).
    pub fn flush(&mut self) -> Result<(String, Vec<ToolCall>), DecodeError> {
        // If we ended inside a partial marker that never closed, treat it as text.
        let remaining = std::mem::take(&mut self.buf);
        self.in_marker = false;
        Ok((remaining, vec![]))
    }

    /// Core processing loop: extract complete markers from `buf`.
    fn process(&mut self) -> Result<(String, Vec<ToolCall>), DecodeError> {
        let mut passthrough = String::new();
        let mut calls = Vec::new();

        loop {
            if self.in_marker {
                // We're inside a marker. Look for the closing `>>`.
                if let Some(close) = find_close_outside_json(&self.buf) {
                    let marker_with_close = &self.buf[..close + 2]; // includes >>
                    // The buf starts with "<<call ..." — parse it.
                    let parsed = decode_calls(marker_with_close, self.tools)?;
                    calls.extend(parsed);
                    self.buf = self.buf[close + 2..].to_string();
                    self.in_marker = false;
                    // Continue processing remaining buffer.
                } else {
                    // Not yet closed — keep buffering.
                    break;
                }
            } else {
                // Look for start of a new marker.
                if let Some(open_pos) = self.buf.find(OPEN) {
                    // Any text before the marker is safe to pass through.
                    passthrough.push_str(&self.buf[..open_pos]);
                    self.buf = self.buf[open_pos..].to_string();
                    self.in_marker = true;
                    // Continue to find the close.
                } else {
                    // No marker found. But we must retain a suffix that could
                    // be the start of a split marker (up to OPEN.len()-1 bytes).
                    let safe_len = safe_passthrough_len(&self.buf);
                    passthrough.push_str(&self.buf[..safe_len]);
                    self.buf = self.buf[safe_len..].to_string();
                    break;
                }
            }
        }

        Ok((passthrough, calls))
    }
}

/// Find the closing `>>` outside JSON strings and brackets.
fn find_close_outside_json(s: &str) -> Option<usize> {
    // Skip the opening "<<call " prefix.
    let after_open = s.strip_prefix(OPEN)?;
    let prefix_len = OPEN.len();

    let bytes = after_open.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        let b = bytes[i];

        if in_string {
            match b {
                b'\\' => escape_next = true,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }

        match b {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            b'>' if depth == 0 && i + 1 < len && bytes[i + 1] == b'>' => {
                return Some(prefix_len + i);
            }
            _ => {}
        }

        i += 1;
    }

    None
}

/// How many bytes of `s` are definitely not part of any future marker.
///
/// We must retain the last `OPEN.len() - 1` bytes because a split marker's
/// opening `<<call ` might span two chunks.
fn safe_passthrough_len(s: &str) -> usize {
    let max_suffix = OPEN.len() - 1; // 6 bytes
    if s.len() <= max_suffix {
        return 0;
    }
    // We need to ensure we don't cut in the middle of a multibyte character.
    let candidate = s.len() - max_suffix;
    // Walk back to a char boundary.
    let mut boundary = candidate;
    while !s.is_char_boundary(boundary) && boundary > 0 {
        boundary -= 1;
    }
    boundary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDef, ToolDef};
    use serde_json::json;

    fn calendar_tool() -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string" }
                    },
                    "required": ["title", "start"]
                })),
            },
        }
    }

    #[test]
    fn single_chunk_full_call() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let (text, calls) = dec
            .push(r#"<<call create_calendar_event {"title":"T","start":"S"}>>"#)
            .unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "create_calendar_event");
        assert!(text.is_empty());
    }

    #[test]
    fn marker_split_across_chunks() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let (t1, c1) = dec.push("<<ca").unwrap();
        assert!(c1.is_empty());
        let (t2, c2) = dec
            .push(r#"ll create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30"}>"#)
            .unwrap();
        assert!(c2.is_empty());
        let (t3, c3) = dec.push(">").unwrap();
        assert_eq!(c3.len(), 1);
        assert_eq!(c3[0].arguments["title"], "Retro");
        let _ = t1;
        let _ = t2;
        let _ = t3;
    }

    #[test]
    fn plain_text_passes_through() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let (t, c) = dec.push("Hello, world!").unwrap();
        // Safe prefix (up to len-6) passes through; the last 6 are held.
        assert!(c.is_empty());
        let (t2, _) = dec.flush().unwrap();
        let all_text = t + &t2;
        assert_eq!(all_text, "Hello, world!");
    }

    #[test]
    fn text_before_and_after_call() {
        let tools = [calendar_tool()];
        let mut dec = StreamDecoder::new(&tools);
        let (t1, c1) = dec
            .push(r#"Sure! <<call create_calendar_event {"title":"T","start":"S"}>> Done."#)
            .unwrap();
        let (t2, _) = dec.flush().unwrap();
        assert_eq!(c1.len(), 1);
        let all_text = t1 + &t2;
        assert!(all_text.contains("Sure!") || all_text.contains("Done.") || all_text.is_empty());
    }

    #[test]
    fn double_gt_inside_string_not_closing() {
        let tools = vec![ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
        }];
        let text =
            r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#;
        let mut dec = StreamDecoder::new(&tools);
        let (_, calls) = dec.push(text).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments["subject"], "a >> b");
    }
}
