//! Compact definition text → tool definitions (`decode_tools`), the inverse of `render`.

use serde_json::{Number, Value};

use crate::error::{CompactError, Result};
use crate::render::{KEYWORDS, is_bare_word};
use crate::schema::{Field, Format, Kind, Object, Range, Ty};
use crate::types::ToolDef;

/// Parses compact definitions (one tool per line) back into tool definitions.
pub(crate) fn definitions(text: &str) -> Result<Vec<ToolDef>> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| {
            tool_line(line).map_err(|reason| CompactError::MalformedDefinitions {
                line: i + 1,
                reason,
            })
        })
        .collect()
}

fn tool_line(line: &str) -> std::result::Result<ToolDef, String> {
    let mut c = Cursor { s: line, i: 0 };
    let name = c.take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'));
    if name.is_empty() {
        return Err("missing tool name".into());
    }
    c.expect(b'(')?;
    let mut obj = c.fields(b')')?;
    c.expect(b')')?;
    obj.closed = c.eat(b'!');
    let description = match c.rest().strip_prefix(" - ") {
        Some(desc) => Some(unescape(desc)?),
        None if c.rest().is_empty() => None,
        None => return Err(format!("unexpected text `{}`", c.rest())),
    };
    let parameters = Ty::plain(Kind::Object(obj)).to_json();
    Ok(ToolDef {
        name: name.to_string(),
        description,
        parameters: Some(parameters),
    })
}

/// A recursive-descent reader over one definition line.
struct Cursor<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Cursor<'a> {
    fn rest(&self) -> &'a str {
        self.s.get(self.i..).unwrap_or_default()
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, b: u8) -> std::result::Result<(), String> {
        if self.eat(b) {
            Ok(())
        } else {
            Err(format!("expected `{}` at column {}", b as char, self.i + 1))
        }
    }

    fn take_while(&mut self, f: impl Fn(u8) -> bool) -> &'a str {
        let start = self.i;
        while self.peek().is_some_and(&f) {
            self.i += 1;
        }
        self.s.get(start..self.i).unwrap_or_default()
    }

    /// `field (", " field)*` up to (not consuming) `close`.
    fn fields(&mut self, close: u8) -> std::result::Result<Object, String> {
        let mut fields = Vec::new();
        if self.peek() != Some(close) {
            loop {
                fields.push(self.field()?);
                if !self.eat(b',') {
                    break;
                }
                self.eat(b' ');
            }
        }
        let mut obj = Object {
            fields,
            closed: false,
        };
        obj.sort_fields();
        Ok(obj)
    }

    fn field(&mut self) -> std::result::Result<Field, String> {
        let name = if self.peek() == Some(b'\'') {
            self.quoted()?
        } else {
            self.take_while(|b| b.is_ascii_alphanumeric() || b == b'_')
                .to_string()
        };
        if name.is_empty() {
            return Err(format!("expected a field name at column {}", self.i + 1));
        }
        let required = !self.eat(b'?');
        self.expect(b':')?;
        Ok(Field {
            name,
            required,
            ty: self.ty()?,
        })
    }

    /// `alt ("|" alt)* ["=" json] [" " quoted]`.
    fn ty(&mut self) -> std::result::Result<Ty, String> {
        let mut alts = vec![self.alt()?];
        while self.eat(b'|') {
            alts.push(self.alt()?);
        }
        let mut ty = combine(alts)?;
        if self.eat(b'=') {
            ty.default = Some(self.json_value()?);
        }
        if self.rest().starts_with(" '") {
            self.i += 1;
            ty.description = Some(self.quoted()?);
        }
        Ok(ty)
    }

    fn alt(&mut self) -> std::result::Result<Alt, String> {
        match self.peek() {
            Some(b'[') => {
                self.i += 1;
                let items = self.ty()?;
                self.expect(b']')?;
                Ok(Alt::Type(Kind::Array(Box::new(items))))
            }
            Some(b'{') => {
                self.i += 1;
                let mut obj = self.fields(b'}')?;
                self.expect(b'}')?;
                obj.closed = self.eat(b'!');
                Ok(Alt::Type(Kind::Object(obj)))
            }
            Some(b'\'') => Ok(Alt::Literal(Value::String(self.quoted()?))),
            _ => {
                let word = self.take_while(|b| !b",)}]|=( '".contains(&b));
                self.keyword_or_literal(word)
            }
        }
    }

    fn keyword_or_literal(&mut self, word: &str) -> std::result::Result<Alt, String> {
        let kind = match word {
            "" => return Err(format!("expected a type at column {}", self.i + 1)),
            "str" => Kind::Str(None),
            "int" => Kind::Int(self.range()?),
            "num" => Kind::Num(self.range()?),
            "bool" => Kind::Bool,
            "any" => Kind::Any,
            "obj" => Kind::AnyObject,
            "null" => return Ok(Alt::Literal(Value::Null)),
            "true" => return Ok(Alt::Literal(Value::Bool(true))),
            "false" => return Ok(Alt::Literal(Value::Bool(false))),
            w => match Format::ALL.into_iter().find(|f| f.keyword() == w) {
                Some(f) => Kind::Str(Some(f)),
                None if is_bare_word(w) && !KEYWORDS.contains(&w) => {
                    return Ok(Alt::Literal(Value::String(w.to_string())));
                }
                None => return Ok(Alt::Literal(Value::Number(number(w)?))),
            },
        };
        Ok(Alt::Type(kind))
    }

    /// Optional `(min..max)` after `int` / `num`.
    fn range(&mut self) -> std::result::Result<Range, String> {
        if !self.eat(b'(') {
            return Ok(Range::default());
        }
        let inner = self.take_while(|b| b != b')');
        self.expect(b')')?;
        let (lo, hi) = inner
            .split_once("..")
            .ok_or_else(|| format!("bad range `{inner}`"))?;
        let bound = |s: &str| {
            if s.is_empty() {
                Ok(None)
            } else {
                number(s).map(Some)
            }
        };
        Ok(Range {
            min: bound(lo)?,
            max: bound(hi)?,
        })
    }

    /// `'...'` with the escapes `render::push_quoted` writes.
    fn quoted(&mut self) -> std::result::Result<String, String> {
        self.expect(b'\'')?;
        let rest = self.rest();
        let mut out = String::new();
        let mut chars = rest.char_indices();
        while let Some((at, c)) = chars.next() {
            match c {
                '\'' => {
                    self.i += at + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, e)) => out.push(unescape_char(e)?),
                    None => break,
                },
                c => out.push(c),
            }
        }
        Err("unterminated quoted string".into())
    }

    /// A JSON default value: a string, array or object (balanced) or a scalar token.
    fn json_value(&mut self) -> std::result::Result<Value, String> {
        let start = self.i;
        let bytes = self.s.as_bytes();
        if matches!(self.peek(), Some(b'"' | b'[' | b'{')) {
            let mut depth = 0usize;
            let mut in_string = false;
            let mut escaped = false;
            while let Some(&b) = bytes.get(self.i) {
                self.i += 1;
                if in_string {
                    match b {
                        _ if escaped => escaped = false,
                        b'\\' => escaped = true,
                        b'"' => in_string = false,
                        _ => {}
                    }
                } else {
                    match b {
                        b'"' => in_string = true,
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                if depth == 0 && !in_string {
                    break;
                }
            }
        } else {
            self.take_while(|b| !b",)}]| ".contains(&b));
        }
        let raw = self.s.get(start..self.i).unwrap_or_default();
        serde_json::from_str(raw).map_err(|e| format!("bad default `{raw}`: {e}"))
    }
}

enum Alt {
    Type(Kind),
    Literal(Value),
}

/// Literals only → enum; one type plus an optional `null` → that type, nullable.
fn combine(alts: Vec<Alt>) -> std::result::Result<Ty, String> {
    if alts.iter().all(|a| matches!(a, Alt::Literal(_))) {
        let values = alts
            .into_iter()
            .filter_map(|a| match a {
                Alt::Literal(v) => Some(v),
                Alt::Type(_) => None,
            })
            .collect();
        return Ok(Ty::plain(Kind::Enum(values)));
    }
    let mut kind = None;
    let mut nullable = false;
    for alt in alts {
        match alt {
            Alt::Literal(Value::Null) => nullable = true,
            Alt::Type(k) if kind.is_none() => kind = Some(k),
            _ => return Err("only `T|null` unions are supported".into()),
        }
    }
    let mut ty = Ty::plain(kind.unwrap_or(Kind::Any));
    ty.nullable = nullable;
    Ok(ty)
}

fn number(s: &str) -> std::result::Result<Number, String> {
    serde_json::from_str::<Number>(s).map_err(|_| format!("`{s}` is not a type or a value"))
}

fn unescape_char(e: char) -> std::result::Result<char, String> {
    match e {
        'n' => Ok('\n'),
        'r' => Ok('\r'),
        't' => Ok('\t'),
        '\\' | '\'' => Ok(e),
        other => Err(format!("unknown escape `\\{other}`")),
    }
}

/// Reverses `render::push_escaped` for a tool description.
fn unescape(s: &str) -> std::result::Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let e = chars.next().ok_or("dangling `\\`")?;
            out.push(unescape_char(e)?);
        } else {
            out.push(c);
        }
    }
    Ok(out)
}
