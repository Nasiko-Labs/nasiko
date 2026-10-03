//! The compact call grammar, and the scanner that recognises it.
//!
//! # Grammar
//!
//! ```text
//! OUTPUT          = { TEXT_OUTSIDE_CALLS | CALL }          ; MULTIPLE_CALLS: any number, in order
//! CALL            = OPENING_MARKER WS+ TOOL_NAME WS* ARGUMENT_OBJECT WS* CLOSING_MARKER
//! OPENING_MARKER  = "<<call"
//! CLOSING_MARKER  = ">>"                                   ; the two '>' must be adjacent
//! TOOL_NAME       = 1*64( ALPHA / DIGIT / "_" / "-" / "." )
//! ARGUMENT_OBJECT = one JSON object (RFC 8259), from '{' to its matching '}'
//! WS              = " " / "\t" / "\n" / "\r"
//! TEXT_OUTSIDE_CALLS = any characters that do not start a CALL
//! ```
//!
//! The model is shown the canonical form `<<call NAME {json}>>` ([`OPEN`], [`CLOSE`],
//! [`render_call`]); the scanner additionally tolerates extra whitespace, never anything
//! else.
//!
//! ## Where a call starts
//!
//! A call starts only at `<<call` followed by whitespace. `<<caller`, `<<x` or a lone `<<`
//! are ordinary text. Once `<<call` + whitespace has been seen the scanner is
//! **committed**: anything that does not continue the grammar is an
//! [`CompactError::InvalidSyntax`], never re-read as text. A stream that ends inside a
//! call is [`CompactError::IncompleteStream`].
//!
//! ## Where a call ends
//!
//! The closing marker is recognised only after the argument object is complete:
//! the scanner tracks `in_string`, `escaped` and brace `depth`, and the object ends
//! at the `}` that brings depth back to 0 outside a string. A `>>` (or `}`) inside a
//! JSON string therefore never ends a call. The scanner only finds the object's
//! boundaries; whether it is valid JSON is decided later by a real JSON parser.
//!
//! ## Streaming
//!
//! The scanner consumes one `char` at a time and keeps all state between calls to
//! [`Scanner::push`], so chunk boundaries can fall anywhere, including inside the
//! markers. At most the 6 characters of a possible opening marker are held back from
//! the text output until they are known to be text or a call.
//!
//! ## Limits
//!
//! Untrusted model output cannot make the scanner allocate without bound: tool names
//! are capped at [`MAX_NAME_LEN`], argument objects at [`MAX_ARGS_BYTES`] and nesting
//! at [`MAX_ARGS_DEPTH`]. Text outside calls is never buffered.

use crate::error::{CompactError, Result};
use crate::types::ToolCall;

/// Canonical opening marker, as shown to the model (includes one space).
pub const OPEN: &str = "<<call ";
/// Closing marker.
pub const CLOSE: &str = ">>";

/// Longest accepted tool name, matching OpenAI's function-name limit.
pub const MAX_NAME_LEN: usize = 64;
/// Largest accepted argument object, in bytes.
pub const MAX_ARGS_BYTES: usize = 1 << 20;
/// Deepest accepted brace nesting inside an argument object.
pub const MAX_ARGS_DEPTH: usize = 64;

/// `OPEN` without its trailing space: the scanner accepts any whitespace after it.
const KEYWORD: &[u8] = b"<<call";

/// Render a call in the canonical grammar: `<<call NAME {json}>>`.
///
/// Arguments are written exactly as given. A non-object value is not replaced: it is
/// rendered as-is so the decoder rejects it, rather than silently becoming `{}`.
pub fn render_call(call: &ToolCall) -> String {
    format!("{OPEN}{} {}{CLOSE}", call.name, call.arguments)
}

/// A syntactic unit of model output. The scanner knows nothing about tools or
/// schemas; `args` is the raw text of the argument object.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Token {
    Text(String),
    Call { name: String, args: String },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    /// Plain text.
    Text,
    /// `matched` bytes of [`KEYWORD`] seen; they are held in `pending`.
    Keyword { matched: usize },
    /// After `<<call` + whitespace: skipping whitespace before the name.
    NameStart,
    /// Reading the tool name.
    Name,
    /// After the name: skipping whitespace before `{`.
    BeforeArgs,
    /// Inside the argument object.
    Args {
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    /// After the argument object: expecting `>>` (`saw_gt` once the first `>` is in).
    AfterArgs { saw_gt: bool },
    /// A syntax error occurred; the scanner accepts no more input.
    Failed,
}

/// Incremental scanner for the compact call grammar.
#[derive(Debug, Clone)]
pub(crate) struct Scanner {
    state: State,
    /// Possible opening-marker prefix, not yet known to be text.
    pending: String,
    /// Text recognised during the current `push`, not yet emitted.
    text: String,
    name: String,
    args: String,
}

impl Scanner {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Text,
            pending: String::new(),
            text: String::new(),
            name: String::new(),
            args: String::new(),
        }
    }

    /// Feed the next chunk. Returns the tokens completed by it, in order.
    pub(crate) fn push(&mut self, chunk: &str) -> Result<Vec<Token>> {
        if self.state == State::Failed {
            return Err(CompactError::InvalidSyntax(
                "scanner already failed".to_string(),
            ));
        }
        let mut out = Vec::new();
        for c in chunk.chars() {
            if let Err(e) = self.step(c, &mut out) {
                self.state = State::Failed;
                return Err(e);
            }
        }
        self.flush_text(&mut out);
        Ok(out)
    }

    /// Signal end of input. A partial opening marker is text; a started call is an error.
    pub(crate) fn finish(&mut self) -> Result<Vec<Token>> {
        let mut out = Vec::new();
        match self.state {
            State::Text | State::Keyword { .. } => {
                let pending = std::mem::take(&mut self.pending);
                self.text.push_str(&pending);
                self.state = State::Text;
                self.flush_text(&mut out);
                Ok(out)
            }
            State::Failed => Err(CompactError::InvalidSyntax(
                "scanner already failed".to_string(),
            )),
            _ => {
                self.state = State::Failed;
                Err(CompactError::IncompleteStream)
            }
        }
    }

    fn step(&mut self, c: char, out: &mut Vec<Token>) -> Result<()> {
        match self.state {
            State::Text => {
                if c == '<' {
                    self.pending.push(c);
                    self.state = State::Keyword { matched: 1 };
                } else {
                    self.text.push(c);
                }
            }
            State::Keyword { matched } if matched < KEYWORD.len() => {
                if c == char::from(KEYWORD[matched]) {
                    self.pending.push(c);
                    self.state = State::Keyword {
                        matched: matched + 1,
                    };
                } else if matched == 2 && c == '<' {
                    // "<<" + "<": the oldest '<' is text, the last two may still open a call.
                    self.text.push('<');
                } else {
                    self.release_pending();
                    return self.step(c, out);
                }
            }
            State::Keyword { .. } => {
                if is_ws(c) {
                    self.pending.clear();
                    self.state = State::NameStart;
                } else {
                    // "<<call" not followed by whitespace (e.g. "<<caller") is text.
                    self.release_pending();
                    return self.step(c, out);
                }
            }
            State::NameStart => {
                if is_name_char(c) {
                    self.name.push(c);
                    self.state = State::Name;
                } else if !is_ws(c) {
                    return Err(syntax(format!(
                        "expected a tool name after '<<call', found {c:?}"
                    )));
                }
            }
            State::Name => {
                if is_name_char(c) {
                    if self.name.len() >= MAX_NAME_LEN {
                        return Err(syntax(format!(
                            "tool name longer than {MAX_NAME_LEN} characters"
                        )));
                    }
                    self.name.push(c);
                } else if is_ws(c) {
                    self.state = State::BeforeArgs;
                } else if c == '{' {
                    self.start_args();
                } else {
                    return Err(syntax(format!(
                        "unexpected {c:?} in tool name '{}'",
                        self.name
                    )));
                }
            }
            State::BeforeArgs => {
                if c == '{' {
                    self.start_args();
                } else if !is_ws(c) {
                    return Err(syntax(format!(
                        "expected '{{' to start the arguments of '{}', found {c:?}",
                        self.name
                    )));
                }
            }
            State::Args {
                depth,
                in_string,
                escaped,
            } => {
                if self.args.len() + c.len_utf8() > MAX_ARGS_BYTES {
                    return Err(syntax(format!(
                        "arguments longer than {MAX_ARGS_BYTES} bytes"
                    )));
                }
                self.args.push(c);
                self.state = if in_string {
                    State::Args {
                        depth,
                        in_string: !(c == '"' && !escaped),
                        escaped: c == '\\' && !escaped,
                    }
                } else {
                    match c {
                        '"' => State::Args {
                            depth,
                            in_string: true,
                            escaped: false,
                        },
                        '{' if depth >= MAX_ARGS_DEPTH => {
                            return Err(syntax(format!(
                                "arguments nested deeper than {MAX_ARGS_DEPTH} levels"
                            )));
                        }
                        '{' => State::Args {
                            depth: depth + 1,
                            in_string: false,
                            escaped: false,
                        },
                        '}' if depth == 1 => State::AfterArgs { saw_gt: false },
                        '}' => State::Args {
                            depth: depth - 1,
                            in_string: false,
                            escaped: false,
                        },
                        _ => self.state,
                    }
                };
            }
            State::AfterArgs { saw_gt: false } => {
                if c == '>' {
                    self.state = State::AfterArgs { saw_gt: true };
                } else if !is_ws(c) {
                    return Err(syntax(format!(
                        "expected '>>' after the arguments of '{}', found {c:?}",
                        self.name
                    )));
                }
            }
            State::AfterArgs { saw_gt: true } => {
                if c != '>' {
                    return Err(syntax(format!(
                        "expected '>>' after the arguments of '{}', found '>' then {c:?}",
                        self.name
                    )));
                }
                self.flush_text(out);
                out.push(Token::Call {
                    name: std::mem::take(&mut self.name),
                    args: std::mem::take(&mut self.args),
                });
                self.state = State::Text;
            }
            State::Failed => unreachable!("push refuses input after failure"),
        }
        Ok(())
    }

    fn start_args(&mut self) {
        self.args.push('{');
        self.state = State::Args {
            depth: 1,
            in_string: false,
            escaped: false,
        };
    }

    /// The held-back marker prefix turned out to be text.
    fn release_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        self.text.push_str(&pending);
        self.state = State::Text;
    }

    fn flush_text(&mut self, out: &mut Vec<Token>) {
        if !self.text.is_empty() {
            out.push(Token::Text(std::mem::take(&mut self.text)));
        }
    }
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

fn syntax(reason: String) -> CompactError {
    CompactError::InvalidSyntax(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scan(chunks: &[&str]) -> Result<Vec<Token>> {
        let mut s = Scanner::new();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(s.push(chunk)?);
        }
        out.extend(s.finish()?);
        Ok(merge_text(out))
    }

    /// Adjacent Text tokens are an artefact of chunking; merge them for comparison.
    fn merge_text(tokens: Vec<Token>) -> Vec<Token> {
        let mut merged: Vec<Token> = Vec::new();
        for t in tokens {
            match (merged.last_mut(), t) {
                (Some(Token::Text(prev)), Token::Text(next)) => prev.push_str(&next),
                (_, t) => merged.push(t),
            }
        }
        merged
    }

    fn call(name: &str, args: &str) -> Token {
        Token::Call {
            name: name.into(),
            args: args.into(),
        }
    }

    fn text(s: &str) -> Token {
        Token::Text(s.into())
    }

    #[test]
    fn open_constant_is_keyword_plus_space() {
        assert_eq!(OPEN.as_bytes(), [KEYWORD, b" "].concat());
    }

    #[test]
    fn render_call_is_canonical() {
        let c = ToolCall {
            name: "send_email".into(),
            arguments: json!({"to": ["a@example.com"]}),
        };
        assert_eq!(
            render_call(&c),
            r#"<<call send_email {"to":["a@example.com"]}>>"#
        );
    }

    #[test]
    fn render_call_does_not_hide_bad_arguments() {
        let c = ToolCall {
            name: "t".into(),
            arguments: json!([1]),
        };
        assert_eq!(render_call(&c), "<<call t [1]>>");
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            scan(&["no calls here"]).unwrap(),
            vec![text("no calls here")]
        );
        assert_eq!(scan(&[""]).unwrap(), vec![]);
    }

    #[test]
    fn single_call_with_surrounding_text() {
        assert_eq!(
            scan(&["Sure. <<call t {\"a\":1}>> Done."]).unwrap(),
            vec![text("Sure. "), call("t", "{\"a\":1}"), text(" Done.")]
        );
    }

    #[test]
    fn marker_split_across_chunks() {
        assert_eq!(
            scan(&["<<ca", "ll t {\"a\":\"x", "\"}>", ">"]).unwrap(),
            vec![call("t", "{\"a\":\"x\"}")]
        );
    }

    #[test]
    fn close_marker_inside_string_is_not_a_close() {
        assert_eq!(
            scan(&[r#"<<call t {"s":"a >> b }"}>>"#]).unwrap(),
            vec![call("t", r#"{"s":"a >> b }"}"#)]
        );
    }

    #[test]
    fn escaped_quote_and_backslash_in_string() {
        let args = r#"{"s":"say \"hi\" >> \\","t":"}"}"#;
        assert_eq!(
            scan(&[&format!("<<call t {args}>>")]).unwrap(),
            vec![call("t", args)]
        );
    }

    #[test]
    fn nested_braces() {
        let args = r#"{"a":{"b":{"c":[1,{"d":2}]}}}"#;
        assert_eq!(
            scan(&[&format!("<<call t {args}>>")]).unwrap(),
            vec![call("t", args)]
        );
    }

    #[test]
    fn extra_whitespace_is_tolerated() {
        assert_eq!(
            scan(&["<<call\n  t\t{}  \n>>"]).unwrap(),
            vec![call("t", "{}")]
        );
        assert_eq!(scan(&["<<call t{}>>"]).unwrap(), vec![call("t", "{}")]);
    }

    #[test]
    fn non_markers_are_text() {
        for s in [
            "<<caller",
            "a << b",
            "<<",
            "<",
            "<<cal",
            "<<call",
            "x <<x>> y",
        ] {
            assert_eq!(scan(&[s]).unwrap(), vec![text(s)], "{s}");
        }
    }

    #[test]
    fn extra_angle_brackets_before_marker() {
        assert_eq!(
            scan(&["<<<call t {}>>"]).unwrap(),
            vec![text("<"), call("t", "{}")]
        );
        assert_eq!(
            scan(&["<<c<<call t {}>>"]).unwrap(),
            vec![text("<<c"), call("t", "{}")]
        );
    }

    #[test]
    fn multiple_calls_in_order() {
        assert_eq!(
            scan(&["<<call a {}>>\n<<call b {\"x\":1}>>"]).unwrap(),
            vec![call("a", "{}"), text("\n"), call("b", "{\"x\":1}")]
        );
    }

    #[test]
    fn committed_call_errors_are_syntax_errors() {
        for bad in [
            "<<call >>",
            "<<call t>>",
            "<<call t x {}>>",
            "<<call t {} > >",
            "<<call t {}>x",
            "<<call t! {}>>",
            "<<call t [1]>>",
        ] {
            assert!(
                matches!(scan(&[bad]), Err(CompactError::InvalidSyntax(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn unterminated_call_is_incomplete() {
        for bad in [
            "<<call ",
            "<<call t",
            "<<call t {\"a\":",
            "<<call t {}",
            "<<call t {}>",
        ] {
            assert_eq!(scan(&[bad]), Err(CompactError::IncompleteStream), "{bad}");
        }
    }

    #[test]
    fn limits_are_enforced() {
        let long_name = "n".repeat(MAX_NAME_LEN + 1);
        assert!(scan(&[&format!("<<call {long_name} {{}}>>")]).is_err());
        let ok_name = "n".repeat(MAX_NAME_LEN);
        assert!(scan(&[&format!("<<call {ok_name} {{}}>>")]).is_ok());

        let deep = format!(
            "<<call t {}{}>>",
            "{".repeat(MAX_ARGS_DEPTH + 1),
            "}".repeat(MAX_ARGS_DEPTH + 1)
        );
        assert!(scan(&[&deep]).is_err());

        let huge = format!("<<call t {{\"s\":\"{}\"}}>>", "x".repeat(MAX_ARGS_BYTES));
        assert!(scan(&[&huge]).is_err());
    }

    #[test]
    fn scanner_refuses_input_after_failure() {
        let mut s = Scanner::new();
        assert!(s.push("<<call !").is_err());
        assert!(s.push("ok").is_err());
        assert!(s.finish().is_err());
    }
}
