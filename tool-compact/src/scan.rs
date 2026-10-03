//! String-aware, escape-aware JSON object scanner.
//!
//! This module implements the single shared parser core used by both
//! [`crate::decode::decode_calls`] and [`crate::decode::StreamDecoder`].
//!
//! # Why a dedicated scanner?
//!
//! The end marker `>>` must be located only **after** the JSON object closes,
//! never inside a string value. A naïve `find(">>")`  fails on arguments like
//! `{"body":"a >> b"}`. This scanner tracks:
//!
//! - whether we are inside a JSON string (`"..."`),
//! - proper escape handling (`\"`, `\\`, `\uXXXX`, and split-byte sequences),
//! - `{}`/`[]` depth so that `>>` is accepted only once the root object closes,
//! - multi-byte UTF-8 that may be split across push boundaries.

use crate::{Error, Result};

/// Maximum configurable nesting depth for JSON values.
pub const DEFAULT_MAX_DEPTH: usize = 64;
/// Maximum byte buffer size per call body.
pub const DEFAULT_MAX_BODY_BYTES: usize = 256 * 1024; // 256 KiB

// ── State machine states ──────────────────────────────────────────────────────

/// Top-level states for the prefix+call scanner.
#[derive(Debug, Clone, PartialEq)]
enum State {
    /// Scanning for the start of `<<call `.
    Scanning,
    /// Consuming bytes of the `<<call ` prefix.  The inner `usize` is the number
    /// of prefix bytes already matched (1..PREFIX.len()-1).
    InPrefix(usize),
    /// Reading the tool name (characters up to the first space after `<<call `).
    InName,
    /// Waiting for the `{` that opens the JSON body (we consumed the space).
    WaitBrace,
    /// Inside the JSON body, scanning depth / strings / escapes.
    InBody {
        /// `{}`/`[]` nesting depth.  Starts at 1 after the opening `{`.
        depth: usize,
        /// Whether we are currently inside a JSON string.
        in_string: bool,
        /// Whether the previous byte was a `\` (escape prefix).
        escaped: bool,
        /// Remaining hex digits in a `\uXXXX` sequence.
        unicode_remaining: u8,
    },
    /// JSON body is closed; waiting for the first `>` of `>>`.
    WaitGt1,
    /// Saw one `>`; waiting for the second `>`.
    WaitGt2,
}

/// The prefix `<<call ` (7 bytes).
const PREFIX: &[u8] = b"<<call ";

/// A completed call found by the scanner.
#[derive(Debug, Clone)]
pub(crate) struct FoundCall {
    /// Raw tool name as read from the stream.
    pub name: String,
    /// Raw JSON object body (including outer `{}`).
    pub body: String,
}

/// Combined prefix+body scanner.
///
/// Feed bytes one at a time with [`PrefixScanner::push_byte`].  When it returns
/// `Ok(Some(FoundCall))`, a complete `<<call NAME {…}>>` has been found.
#[derive(Debug, Clone)]
pub(crate) struct PrefixScanner {
    state: State,
    /// Accumulated tool name bytes.
    name_buf: Vec<u8>,
    /// Accumulated JSON body bytes (including the outer `{}`).
    body_buf: Vec<u8>,
}

impl PrefixScanner {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Scanning,
            name_buf: Vec::new(),
            body_buf: Vec::new(),
        }
    }

    /// Returns true when the scanner is mid-name or mid-body (unterminated call).
    pub(crate) fn in_progress(&self) -> bool {
        matches!(
            self.state,
            State::InName | State::WaitBrace | State::InBody { .. } | State::WaitGt1 | State::WaitGt2
        )
    }

    /// Push one byte; returns `Ok(Some(FoundCall))` when a complete call is found.
    pub(crate) fn push_byte(
        &mut self,
        byte: u8,
        max_body_bytes: usize,
        max_depth: usize,
    ) -> Result<Option<FoundCall>> {
        loop {
            // Use a match that may `continue` when we need to re-process the byte
            // in a new state (e.g. after detecting a prefix mismatch and resetting).
            match &self.state {
                // ── Scanning for `<<` ─────────────────────────────────────────
                State::Scanning => {
                    if byte == b'<' {
                        self.state = State::InPrefix(1);
                    }
                    return Ok(None);
                }

                // ── Matching `<<call ` byte by byte ───────────────────────────
                State::InPrefix(n) => {
                    let n = *n;
                    if byte == PREFIX[n] {
                        if n + 1 == PREFIX.len() {
                            // Full prefix matched — move to reading the name.
                            self.state = State::InName;
                            self.name_buf.clear();
                            self.body_buf.clear();
                        } else {
                            self.state = State::InPrefix(n + 1);
                        }
                        return Ok(None);
                    }
                    // Mismatch.  Reset and re-examine this byte from Scanning.
                    self.state = State::Scanning;
                    continue; // re-process `byte` in Scanning state
                }

                // ── Reading tool name ─────────────────────────────────────────
                State::InName => {
                    if byte == b' ' {
                        // Name complete; now expect `{`.
                        self.state = State::WaitBrace;
                        return Ok(None);
                    }
                    // Non-space, non-`{` byte → accumulate into name.
                    self.name_buf.push(byte);
                    return Ok(None);
                }

                // ── Waiting for the opening `{` ───────────────────────────────
                State::WaitBrace => {
                    if byte == b'{' {
                        self.body_buf.push(byte);
                        self.state = State::InBody {
                            depth: 1,
                            in_string: false,
                            escaped: false,
                            unicode_remaining: 0,
                        };
                        return Ok(None);
                    }
                    // Not a `{` immediately after the name — not a valid call.
                    self.state = State::Scanning;
                    self.name_buf.clear();
                    return Ok(None);
                }

                // ── Inside the JSON body ──────────────────────────────────────
                State::InBody {
                    depth,
                    in_string,
                    escaped,
                    unicode_remaining,
                } => {
                    if self.body_buf.len() >= max_body_bytes {
                        self.state = State::Scanning;
                        self.name_buf.clear();
                        self.body_buf.clear();
                        return Err(Error::BodyTooLarge { limit: max_body_bytes });
                    }

                    // Clone the fields so we can mutate them.
                    let mut depth = *depth;
                    let mut in_string = *in_string;
                    let mut escaped = *escaped;
                    let mut unicode_remaining = *unicode_remaining;

                    self.body_buf.push(byte);

                    // ── Unicode escape continuation ──────────────────────────
                    if unicode_remaining > 0 {
                        unicode_remaining -= 1;
                        if unicode_remaining == 0 {
                            escaped = false;
                        }
                        self.state = State::InBody {
                            depth,
                            in_string,
                            escaped,
                            unicode_remaining,
                        };
                        return Ok(None);
                    }

                    // ── Escape prefix ────────────────────────────────────────
                    if escaped {
                        if byte == b'u' {
                            unicode_remaining = 4;
                        } else {
                            escaped = false;
                        }
                        self.state = State::InBody {
                            depth,
                            in_string,
                            escaped,
                            unicode_remaining,
                        };
                        return Ok(None);
                    }

                    // ── Inside string ────────────────────────────────────────
                    if in_string {
                        match byte {
                            b'\\' => escaped = true,
                            b'"' => in_string = false,
                            _ => {}
                        }
                        self.state = State::InBody {
                            depth,
                            in_string,
                            escaped,
                            unicode_remaining,
                        };
                        return Ok(None);
                    }

                    // ── Outside string ───────────────────────────────────────
                    match byte {
                        b'"' => in_string = true,
                        b'{' | b'[' => {
                            depth += 1;
                            if depth > max_depth {
                                self.state = State::Scanning;
                                self.name_buf.clear();
                                self.body_buf.clear();
                                return Err(Error::DepthExceeded { limit: max_depth });
                            }
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                // Root object closed; now wait for `>>`.
                                self.state = State::WaitGt1;
                                return Ok(None);
                            }
                        }
                        _ => {}
                    }

                    self.state = State::InBody {
                        depth,
                        in_string,
                        escaped,
                        unicode_remaining,
                    };
                    return Ok(None);
                }

                // ── Waiting for `>` (first) ───────────────────────────────────
                State::WaitGt1 => {
                    if byte == b'>' {
                        self.state = State::WaitGt2;
                    } else {
                        // Unexpected byte after JSON close; abort this call.
                        self.state = State::Scanning;
                        self.name_buf.clear();
                        self.body_buf.clear();
                    }
                    return Ok(None);
                }

                // ── Waiting for `>` (second) ──────────────────────────────────
                State::WaitGt2 => {
                    if byte == b'>' {
                        // Complete call found!
                        let name = String::from_utf8_lossy(&self.name_buf).into_owned();
                        let body = String::from_utf8_lossy(&self.body_buf).into_owned();
                        let result = if is_valid_name(&name) {
                            Some(FoundCall { name, body })
                        } else {
                            None
                        };
                        self.state = State::Scanning;
                        self.name_buf.clear();
                        self.body_buf.clear();
                        return Ok(result);
                    }
                    // Only one `>` — not the end marker; back to waiting for gt1.
                    // This byte might itself be a `>`.
                    self.state = if byte == b'>' {
                        State::WaitGt2
                    } else {
                        State::WaitGt1
                    };
                    return Ok(None);
                }
            }
        }
    }
}

impl Default for PrefixScanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Validate that a name matches `[A-Za-z_][A-Za-z0-9_.-]*`.
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_string(s: &str) -> Result<Option<FoundCall>> {
        let mut scanner = PrefixScanner::new();
        let mut result = None;
        for b in s.bytes() {
            if let Some(call) = scanner.push_byte(b, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH)? {
                result = Some(call);
            }
        }
        Ok(result)
    }

    #[test]
    fn simple_call_found() {
        let call = scan_string(r#"<<call ping {}>>"#).unwrap().unwrap();
        assert_eq!(call.name, "ping");
        assert_eq!(call.body, "{}");
    }

    #[test]
    fn call_with_args() {
        let call = scan_string(r#"<<call send_email {"to":"alice","subject":"hi"}>>"#)
            .unwrap()
            .unwrap();
        assert_eq!(call.name, "send_email");
        assert!(call.body.contains("alice"));
    }

    #[test]
    fn gt_inside_string_does_not_end_call() {
        let call = scan_string(r#"<<call f {"body":"a >> b"}>>"#)
            .unwrap()
            .unwrap();
        assert_eq!(call.name, "f");
        assert!(call.body.contains("a >> b"));
    }

    #[test]
    fn text_before_and_after_call() {
        let call = scan_string(r#"hello <<call ping {}>> world"#).unwrap().unwrap();
        assert_eq!(call.name, "ping");
    }

    #[test]
    fn stray_lt_is_not_an_error() {
        let result = scan_string("hello < world").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn double_lt_not_followed_by_call_is_not_an_error() {
        let result = scan_string("<< not a call").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn nested_json_works() {
        let s = r#"<<call f {"a":{"b":1}}>>"#;
        let call = scan_string(s).unwrap().unwrap();
        assert_eq!(call.body, r#"{"a":{"b":1}}"#);
    }

    #[test]
    fn valid_name_chars() {
        assert!(is_valid_name("abc"));
        assert!(is_valid_name("_foo"));
        assert!(is_valid_name("foo_bar-baz.v2"));
        assert!(!is_valid_name("1abc"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("-foo"));
    }

    #[test]
    fn body_too_large_returns_error() {
        let mut scanner = PrefixScanner::new();
        // Feed the prefix.
        for b in b"<<call ping " {
            let _ = scanner.push_byte(*b, 10, DEFAULT_MAX_DEPTH);
        }
        // Feed `{` to enter body.
        let _ = scanner.push_byte(b'{', 10, DEFAULT_MAX_DEPTH);
        // Feed enough bytes to exceed limit of 10.
        let result = (0..20).try_fold(None, |_, _| {
            scanner.push_byte(b'x', 10, DEFAULT_MAX_DEPTH)
        });
        assert!(matches!(result, Err(Error::BodyTooLarge { .. })));
    }

    #[test]
    fn scan_split_at_every_boundary() {
        let text = r#"<<call send_email {"to":"x@x.com","subject":"hi"}>>"#;
        let expected = scan_string(text).unwrap().unwrap();

        for split in 1..text.len() {
            let mut scanner = PrefixScanner::new();
            let mut found = None;
            for b in text[..split].bytes() {
                if let Some(c) = scanner
                    .push_byte(b, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH)
                    .unwrap()
                {
                    found = Some(c);
                }
            }
            for b in text[split..].bytes() {
                if let Some(c) = scanner
                    .push_byte(b, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_DEPTH)
                    .unwrap()
                {
                    found = Some(c);
                }
            }
            let got = found.unwrap_or_else(|| panic!("no call found at split {split}"));
            assert_eq!(got.body, expected.body, "body differs at split {split}");
            assert_eq!(got.name, expected.name, "name differs at split {split}");
        }
    }
}
