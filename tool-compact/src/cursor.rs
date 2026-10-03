//! A char-boundary-safe cursor. Every advance is by the length of a matched `&str`, so `pos` is
//! always a char boundary and no byte-index slicing is needed.

pub(crate) struct Cursor<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    /// Byte offset into the source.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    /// Everything not yet consumed.
    pub(crate) fn rest(&self) -> &'a str {
        self.src.get(self.pos..).unwrap_or("")
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest().is_empty()
    }

    pub(crate) fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    /// Consume `prefix` if the rest starts with it.
    pub(crate) fn eat(&mut self, prefix: &str) -> bool {
        if self.rest().starts_with(prefix) {
            self.pos += prefix.len();
            true
        } else {
            false
        }
    }

    /// Consume the longest prefix whose chars all satisfy `pred`.
    pub(crate) fn take_while(&mut self, pred: impl Fn(char) -> bool) -> &'a str {
        let rest = self.rest();
        let end = rest
            .char_indices()
            .find(|&(_, c)| !pred(c))
            .map_or(rest.len(), |(i, _)| i);
        let taken = rest.get(..end).unwrap_or("");
        self.pos += taken.len();
        taken
    }

    pub(crate) fn skip_ws(&mut self) -> usize {
        self.take_while(char::is_whitespace).len()
    }

    /// Advance by `n` bytes of the rest; only call with a length that ends on a char boundary
    /// (e.g. a serde_json byte offset). Refuses a non-boundary instead of panicking.
    pub(crate) fn advance(&mut self, n: usize) -> bool {
        if self.rest().is_char_boundary(n) && n <= self.rest().len() {
            self.pos += n;
            true
        } else {
            false
        }
    }
}
