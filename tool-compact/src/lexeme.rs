//! Lexical rules shared by the renderer and the parser, and the only module that slices text.
//!
//! Every slice goes through [`Cursor`], which only ever cuts at byte offsets it has reached by
//! iterating chars, so `str::get` never returns `None` for a well-formed position; when it does
//! (a bug), the cursor reports a parse error instead of panicking.

use serde_json::{Number, Value};

use crate::limits::MAX_NAME_LEN;

/// Words that cannot be bare enum members because the parser would read them as types or
/// literals.
pub(crate) const RESERVED: &[&str] = &[
    "str", "int", "num", "bool", "null", "datetime", "date", "time", "email", "uri", "uuid",
    "true", "false",
];

/// `format` values that render as a type word instead of `str(format=…)`.
pub(crate) const FORMAT_ALIASES: &[(&str, &str)] = &[
    ("date-time", "datetime"),
    ("date", "date"),
    ("time", "time"),
    ("email", "email"),
    ("uri", "uri"),
    ("uuid", "uuid"),
];

pub(crate) fn alias_for_format(format: &str) -> Option<&'static str> {
    FORMAT_ALIASES
        .iter()
        .find(|(f, _)| *f == format)
        .map(|(_, alias)| *alias)
}

pub(crate) fn format_for_alias(alias: &str) -> Option<&'static str> {
    FORMAT_ALIASES
        .iter()
        .find(|(_, a)| *a == alias)
        .map(|(f, _)| *f)
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

/// Tool names: `[A-Za-z0-9_.-]{1,64}` (the OpenAI rule, which permits a leading digit).
pub(crate) fn is_tool_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME_LEN && s.chars().all(is_name_char)
}

/// Property names the notation can carry bare: `[A-Za-z_][A-Za-z0-9_.-]{0,63}`.
pub(crate) fn is_arg_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => s.len() <= MAX_NAME_LEN && chars.all(is_name_char),
        _ => false,
    }
}

/// Enum members and `format` values that render without quotes.
pub(crate) fn is_bare_word(s: &str) -> bool {
    is_arg_name(s) && !RESERVED.contains(&s)
}

/// JSON string literal for `s`, escaping only `"`, `\` and control characters.
pub(crate) fn quote(s: &str) -> String {
    // Serializing a `&str` cannot fail: there is no I/O and no non-string key involved.
    serde_json::to_string(s).unwrap_or_else(|_| String::from("\"\""))
}

/// Property and root-schema descriptions sit between delimiters, so they are always quoted.
/// Single quotes are preferred: the compact text travels inside a JSON string, where every `"`
/// costs an escape. A description that contains `'`, `\` or a control character falls back to
/// the JSON form, so the two forms partition the strings and parsing is unambiguous.
pub(crate) fn quote_description(s: &str) -> String {
    if !s.is_empty() && !s.contains('\'') && !s.contains('\\') && !s.chars().any(char::is_control) {
        format!("'{s}'")
    } else {
        quote(s)
    }
}

/// A tool description can stay raw when it contains no control characters, no surrounding
/// whitespace and does not start with a quote (the parser dispatches on the first character).
pub(crate) fn is_raw_safe(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('"')
        && !s.starts_with(char::is_whitespace)
        && !s.ends_with(char::is_whitespace)
        && !s.chars().any(|c| c.is_control())
}

/// Byte length of the JSON number at the start of `s` (RFC 8259 grammar), or 0. A `.` or `e`
/// only belongs to the number when a digit follows, so `0..10` scans as `0` then `..10`.
fn number_prefix_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    if !digits(&mut i) {
        return 0;
    }
    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        digits(&mut i);
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            i = j;
            digits(&mut i);
        }
    }
    i
}

/// Position-tracking reader over one line of compact notation.
pub(crate) struct Cursor<'a> {
    text: &'a str,
    pos: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError {
    pub pos: usize,
    pub reason: String,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn err(&self, reason: impl Into<String>) -> ParseError {
        ParseError {
            pos: self.pos,
            reason: reason.into(),
        }
    }

    pub(crate) fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or("")
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.text.len()
    }

    pub(crate) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    pub(crate) fn starts_with(&self, lit: &str) -> bool {
        self.rest().starts_with(lit)
    }

    /// Consume `lit` if it is next.
    pub(crate) fn eat(&mut self, lit: &str) -> bool {
        if self.starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, lit: &str) -> Result<(), ParseError> {
        if self.eat(lit) {
            Ok(())
        } else {
            Err(self.err(format!("expected {lit:?}")))
        }
    }

    /// Consume the longest prefix of chars matching `pred`.
    pub(crate) fn take_while(&mut self, pred: impl Fn(char) -> bool) -> &'a str {
        let rest = self.rest();
        let end = rest
            .char_indices()
            .find(|(_, c)| !pred(*c))
            .map_or(rest.len(), |(i, _)| i);
        self.pos += end;
        rest.get(..end).unwrap_or("")
    }

    pub(crate) fn take_name(&mut self) -> &'a str {
        self.take_while(is_name_char)
    }

    /// Parse a JSON string literal starting at the cursor.
    pub(crate) fn take_json_string(&mut self) -> Result<String, ParseError> {
        let rest = self.rest();
        if !rest.starts_with('"') {
            return Err(self.err("expected a JSON string"));
        }
        let mut escaped = false;
        let mut end = None;
        for (i, c) in rest.char_indices().skip(1) {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                end = Some(i + 1);
                break;
            }
        }
        let Some(end) = end else {
            return Err(self.err("unterminated JSON string"));
        };
        let literal = rest.get(..end).unwrap_or("");
        let value: String = serde_json::from_str(literal)
            .map_err(|e| self.err(format!("invalid JSON string: {e}")))?;
        self.pos += end;
        Ok(value)
    }

    /// Parse a description: `'…'` (no escapes by construction) or a JSON string.
    pub(crate) fn take_description(&mut self) -> Result<String, ParseError> {
        if self.starts_with("'") {
            let rest = self.rest();
            let inner = rest.get(1..).unwrap_or("");
            let Some(end) = inner.find('\'') else {
                return Err(self.err("unterminated description"));
            };
            let text = inner.get(..end).unwrap_or("").to_owned();
            if text.is_empty() {
                return Err(self.err("single-quoted descriptions are never empty"));
            }
            self.pos += end + 2;
            Ok(text)
        } else {
            self.take_json_string()
        }
    }

    /// Parse a JSON number token with `serde_json`'s number grammar.
    pub(crate) fn take_number(&mut self) -> Result<Number, ParseError> {
        let start = self.pos;
        let rest = self.rest();
        let len = number_prefix_len(rest);
        if len == 0 {
            return Err(self.err("expected a number"));
        }
        let token = rest.get(..len).unwrap_or("");
        self.pos += len;
        serde_json::from_str::<Number>(token).map_err(|e| ParseError {
            pos: start,
            reason: format!("invalid number: {e}"),
        })
    }

    /// Parse one JSON value starting at the cursor, consuming exactly its bytes.
    pub(crate) fn take_json_value(&mut self) -> Result<Value, ParseError> {
        let rest = self.rest();
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        match stream.next() {
            Some(Ok(value)) => {
                self.pos += stream.byte_offset();
                Ok(value)
            }
            Some(Err(e)) => Err(self.err(format!("invalid JSON value: {e}"))),
            None => Err(self.err("expected a JSON value")),
        }
    }

    pub(crate) fn take_u64(&mut self) -> Result<u64, ParseError> {
        let token = self.take_while(|c| c.is_ascii_digit());
        token
            .parse::<u64>()
            .map_err(|_| self.err("expected an unsigned integer"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_follow_the_openai_rule() {
        assert!(is_tool_name("get_weather"));
        assert!(is_tool_name("1st.tool-v2"));
        assert!(!is_tool_name(""));
        assert!(!is_tool_name("has space"));
        assert!(!is_tool_name(&"a".repeat(65)));
        assert!(is_tool_name(&"a".repeat(64)));
    }

    #[test]
    fn arg_names_cannot_start_with_a_digit_and_bare_words_exclude_reserved() {
        assert!(is_arg_name("_x"));
        assert!(!is_arg_name("1x"));
        assert!(is_bare_word("public"));
        assert!(!is_bare_word("str"));
        assert!(!is_bare_word("2"));
        assert!(!is_bare_word("zh-Hans ok"));
    }

    #[test]
    fn descriptions_prefer_single_quotes_and_fall_back_to_json_exactly_when_needed() {
        for (input, expected) in [
            ("Event title", "'Event title'"),
            ("has \"double\" quotes", "'has \"double\" quotes'"),
            ("it's", "\"it's\""),
            ("back\\slash", "\"back\\\\slash\""),
            ("two\nlines", "\"two\\nlines\""),
            ("", "\"\""),
            ("日本語 🎉", "'日本語 🎉'"),
        ] {
            let rendered = quote_description(input);
            assert_eq!(rendered, expected, "{input:?}");
            let mut c = Cursor::new(&rendered);
            assert_eq!(c.take_description().unwrap(), input, "{input:?}");
            assert!(c.at_end());
        }
        assert!(Cursor::new("'open").take_description().is_err());
    }

    #[test]
    fn raw_descriptions_exclude_exactly_what_quoting_handles() {
        assert!(is_raw_safe("Create an event."));
        assert!(!is_raw_safe(" leading"));
        assert!(!is_raw_safe("trailing "));
        assert!(!is_raw_safe("two\nlines"));
        assert!(!is_raw_safe("\"quoted\" start"));
        assert!(!is_raw_safe(""));
        assert!(is_raw_safe("has \"inner\" quotes"));
    }

    #[test]
    fn cursor_reads_json_strings_numbers_and_values_exactly() {
        let mut c = Cursor::new("\"a \\\"b\\\" >> c\" 1.50 {\"k\":[1,2]}tail");
        assert_eq!(c.take_json_string().unwrap(), "a \"b\" >> c");
        assert!(c.eat(" "));
        assert_eq!(c.take_number().unwrap().to_string(), "1.5");
        assert!(c.eat(" "));
        assert_eq!(
            c.take_json_value().unwrap(),
            serde_json::json!({"k": [1, 2]})
        );
        assert_eq!(c.rest(), "tail");
    }

    #[test]
    fn cursor_reports_unterminated_strings_without_panicking() {
        let mut c = Cursor::new("\"open");
        assert!(c.take_json_string().is_err());
        let mut c = Cursor::new("x");
        assert!(c.take_number().is_err());
    }
}
