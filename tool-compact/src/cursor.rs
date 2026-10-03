/// Byte cursor over a UTF-8 string. Indexes stay on char boundaries.
pub(crate) struct Cur<'a> {
    pub s: &'a str,
    pub i: usize,
}

impl<'a> Cur<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    pub(crate) fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    pub(crate) fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    pub(crate) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    pub(crate) fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += ch.len_utf8();
        Some(ch)
    }

    pub(crate) fn starts(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    pub(crate) fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn eat_str(&mut self, prefix: &str) -> bool {
        if self.starts(prefix) {
            self.i += prefix.len();
            true
        } else {
            false
        }
    }

    pub(crate) fn skip_ws(&mut self) -> bool {
        let start = self.i;
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        self.i > start
    }
}
