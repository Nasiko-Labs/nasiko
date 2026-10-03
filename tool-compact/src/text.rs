//! Lexical rules shared by the encoder and the definition parser, so the two cannot disagree
//! about what needs quoting.

/// Type keywords. An enum value or key spelled like one is quoted so it cannot be misread.
pub(crate) const KEYWORDS: [&str; 7] = ["str", "int", "num", "bool", "obj", "datetime", "null"];

/// Whether `s` can be written bare as a key or enum value.
pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(is_ident_char)
}

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Characters allowed in a tool name. A superset of OpenAI's `[a-zA-Z0-9_-]`.
pub(crate) fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

pub(crate) fn is_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_name_char)
}

/// Whether `s` reads as an integer literal: an optional `-`, then digits.
pub(crate) fn is_int_like(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Single-quoted literal. Single quotes because the compact text travels inside a JSON string,
/// where every `"` would cost an escape.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A character cursor. Parsing walks `char`s so nothing ever slices a `str`.
pub(crate) struct Cursor {
    chars: Vec<char>,
    pos: usize,
}

impl Cursor {
    pub(crate) fn new(s: &str) -> Self {
        Self {
            chars: s.chars().collect(),
            pos: 0,
        }
    }

    pub(crate) fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    pub(crate) fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    pub(crate) fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    pub(crate) fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    pub(crate) fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, expected: char) -> Result<(), String> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(match self.peek() {
                Some(found) => format!("expected `{expected}`, found `{found}`"),
                None => format!("expected `{expected}`, found end of line"),
            })
        }
    }

    /// Whether the cursor is at a `|null` marker (and not at a longer word such as `|nullable`).
    pub(crate) fn at_null(&self) -> bool {
        "|null"
            .chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
            && !self.peek_at(5).is_some_and(is_ident_char)
    }

    pub(crate) fn eat_null(&mut self) -> bool {
        let found = self.at_null();
        if found {
            self.pos += 5;
        }
        found
    }

    /// Everything from here up to the next `stop`, without moving.
    pub(crate) fn peek_until(&self, stop: char) -> String {
        self.chars
            .iter()
            .skip(self.pos)
            .take_while(|c| **c != stop)
            .collect()
    }

    pub(crate) fn skip_spaces(&mut self) {
        while self.eat(' ') {}
    }

    pub(crate) fn take_while(&mut self, keep: impl Fn(char) -> bool) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(|c| keep(*c)) {
            out.push(c);
            self.pos += 1;
        }
        out
    }

    pub(crate) fn rest(&mut self) -> String {
        let out = self.chars.iter().skip(self.pos).collect();
        self.pos = self.chars.len();
        out
    }

    /// Read a literal written by [`quote`].
    pub(crate) fn quoted(&mut self) -> Result<String, String> {
        self.expect('\'')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err("unterminated quoted text".into()),
                Some('\'') => return Ok(out),
                Some('\\') => out.push(self.escape()?),
                Some(c) => out.push(c),
            }
        }
    }

    fn escape(&mut self) -> Result<char, String> {
        match self.bump() {
            Some('\\') => Ok('\\'),
            Some('\'') => Ok('\''),
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('t') => Ok('\t'),
            Some('u') => {
                self.expect('{')?;
                let hex = self.take_while(|c| c.is_ascii_hexdigit());
                self.expect('}')?;
                u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("bad escape `\\u{{{hex}}}`"))
            }
            Some(other) => Err(format!("unknown escape `\\{other}`")),
            None => Err("unterminated escape".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_round_trips_awkward_text() {
        for s in [
            "",
            "plain",
            "user's calendar",
            "back\\slash",
            "line\nbreak\r\ttab",
            "bell\u{7}",
            "日本語 · café",
            "''",
        ] {
            let quoted = quote(s);
            assert!(!quoted.contains('\n'), "a literal must stay on one line");
            let mut cursor = Cursor::new(&quoted);
            assert_eq!(cursor.quoted().unwrap(), s);
            assert!(cursor.at_end());
        }
    }

    #[test]
    fn unterminated_or_unknown_escapes_are_errors() {
        for bad in ["'abc", "'a\\", "'a\\q'", "'\\u{110000}'", "'\\u{}'"] {
            assert!(Cursor::new(bad).quoted().is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn ident_and_int_rules() {
        assert!(is_ident("duration_min"));
        assert!(is_ident("_x-1"));
        assert!(!is_ident("1st"));
        assert!(!is_ident("-x"));
        assert!(!is_ident(""));
        assert!(!is_ident("a b"));
        assert!(is_int_like("42"));
        assert!(is_int_like("-7"));
        assert!(!is_int_like("-"));
        assert!(!is_int_like("4.2"));
    }
}
