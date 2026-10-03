//! Parsing: compact definitions text → schema tree. The inverse of [`crate::encode`].
//!
//! Exists so `decode_tools` can prove, mechanically, that every piece of schema meaning the
//! encoder kept is actually present in the text the model sees — not in a side channel.

use serde_json::{Number, Value};

use crate::Error;
use crate::schema::{Ann, Base, Kind, Node, Obj, Prop, is_valid_prop_name};

/// One parsed tool: name, description, parameters object.
pub(crate) struct ParsedTool {
    pub name: String,
    pub description: Option<String>,
    pub params: Obj,
}

fn err(line: usize, reason: impl Into<String>) -> Error {
    Error::MalformedDefinitions {
        line,
        reason: reason.into(),
    }
}

/// A character cursor over one line's type expression.
struct Cursor<'a> {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    _src: &'a str,
}

/// One member of a `|` union before it is classified.
enum Member {
    Base(Base),
    Any,
    Obj,
    Literal(Value),
}

impl<'a> Cursor<'a> {
    fn new(src: &'a str, line: usize) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line,
            _src: src,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn expect(&mut self, c: char) -> Result<(), Error> {
        match self.bump() {
            Some(got) if got == c => Ok(()),
            got => Err(err(self.line, format!("expected '{c}', found {got:?}"))),
        }
    }

    fn rest(&self) -> String {
        self.chars[self.pos..].iter().collect()
    }

    fn word(&mut self) -> String {
        let mut w = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                w.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        w
    }

    fn quoted(&mut self) -> Result<String, Error> {
        self.expect('\'')?;
        let mut s = String::new();
        loop {
            match self.bump() {
                None => return Err(err(self.line, "unterminated string literal")),
                Some('\'') => return Ok(s),
                Some('\\') => match self.bump() {
                    Some('\\') => s.push('\\'),
                    Some('\'') => s.push('\''),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    other => return Err(err(self.line, format!("bad escape {other:?}"))),
                },
                Some(c) => s.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Number, Error> {
        let mut tok = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E') {
                tok.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        serde_json::from_str::<Number>(&tok)
            .map_err(|_| err(self.line, format!("bad number '{tok}'")))
    }

    /// A scalar literal: `'string'`, integer, `true`, `false` or `null`.
    fn literal(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some('\'') => Ok(Value::String(self.quoted()?)),
            Some(c) if c.is_ascii_digit() || c == '-' => Ok(Value::Number(self.number()?)),
            _ => match self.word().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                "null" => Ok(Value::Null),
                w => Err(err(self.line, format!("expected a literal, found '{w}'"))),
            },
        }
    }

    fn member(&mut self) -> Result<Member, Error> {
        match self.peek() {
            Some('\'') => Ok(Member::Literal(Value::String(self.quoted()?))),
            Some(c) if c.is_ascii_digit() || c == '-' => {
                let n = self.number()?;
                if !(n.is_i64() || n.is_u64()) {
                    return Err(err(self.line, "enum literals must be integers"));
                }
                Ok(Member::Literal(Value::Number(n)))
            }
            _ => {
                let w = self.word();
                if let Some(b) = Base::from_keyword(&w) {
                    return Ok(Member::Base(b));
                }
                match w.as_str() {
                    "any" => Ok(Member::Any),
                    "obj" => Ok(Member::Obj),
                    "true" => Ok(Member::Literal(Value::Bool(true))),
                    "false" => Ok(Member::Literal(Value::Bool(false))),
                    _ => Err(err(self.line, format!("unknown type '{w}'"))),
                }
            }
        }
    }

    /// `type := union postfix*`, where `postfix := '[]' ann?`.
    fn parse_type(&mut self) -> Result<Node, Error> {
        let mut node = self.parse_union()?;
        while self.peek() == Some('[') {
            self.bump();
            self.expect(']')?;
            let item = if node.kind == Kind::Any && node.ann.is_empty() {
                None
            } else {
                Some(Box::new(node))
            };
            node = Node {
                kind: Kind::Array(item),
                description: None,
                ann: Ann::default(),
            };
            if self.peek() == Some('(') {
                self.parse_ann(&mut node)?;
            }
        }
        Ok(node)
    }

    /// `union := '(' type ')' | member ('|' member)* ann?`.
    fn parse_union(&mut self) -> Result<Node, Error> {
        if self.peek() == Some('(') {
            self.bump();
            let inner = self.parse_type()?;
            self.expect(')')?;
            return Ok(inner);
        }
        let mut members = vec![self.member()?];
        while self.peek() == Some('|') {
            self.bump();
            members.push(self.member()?);
        }
        let has_literal = members.iter().any(|m| matches!(m, Member::Literal(_)));
        let kind = if has_literal {
            let mut values = Vec::with_capacity(members.len());
            for m in members {
                values.push(match m {
                    Member::Literal(v) => v,
                    // `null` lexes as the base type; inside a literal union it is the literal.
                    Member::Base(Base::Null) => Value::Null,
                    _ => return Err(err(self.line, "type name inside an enum")),
                });
            }
            Kind::Enum(values)
        } else if members.len() == 1 {
            match members.pop() {
                Some(Member::Any) => Kind::Any,
                Some(Member::Obj) => Kind::Object(Obj::default()),
                Some(Member::Base(b)) => Kind::Types(vec![b]),
                _ => return Err(err(self.line, "empty type")),
            }
        } else {
            let mut bases = Vec::with_capacity(members.len());
            for m in members {
                match m {
                    Member::Base(b) => bases.push(b),
                    _ => return Err(err(self.line, "'any' or 'obj' inside a union")),
                }
            }
            Kind::Types(bases)
        };
        let mut node = Node {
            kind,
            description: None,
            ann: Ann::default(),
        };
        if self.peek() == Some('(') {
            self.parse_ann(&mut node)?;
        }
        Ok(node)
    }

    /// `ann := '(' item (',' item)* ')'`, `item := 'closed' | FORMAT | KEY '=' literal`.
    fn parse_ann(&mut self, node: &mut Node) -> Result<(), Error> {
        self.expect('(')?;
        let line = self.line;
        loop {
            let key = self.word();
            if key.is_empty() {
                return Err(err(line, "empty annotation"));
            }
            let a = &mut node.ann;
            if self.peek() == Some('=') {
                self.bump();
                fn set<T>(slot: &mut Option<T>, v: T, key: &str, line: usize) -> Result<(), Error> {
                    if slot.is_some() {
                        return Err(err(line, format!("duplicate annotation '{key}'")));
                    }
                    *slot = Some(v);
                    Ok(())
                }
                let count = |c: &mut Self| -> Result<u64, Error> {
                    c.number()?
                        .as_u64()
                        .ok_or_else(|| err(line, format!("{key} must be a non-negative integer")))
                };
                match key.as_str() {
                    "min" => {
                        let n = self.number()?;
                        set(&mut a.minimum, n, &key, line)?
                    }
                    "max" => {
                        let n = self.number()?;
                        set(&mut a.maximum, n, &key, line)?
                    }
                    "minLen" => {
                        let n = count(self)?;
                        set(&mut node.ann.min_length, n, &key, line)?
                    }
                    "maxLen" => {
                        let n = count(self)?;
                        set(&mut node.ann.max_length, n, &key, line)?
                    }
                    "minItems" => {
                        let n = count(self)?;
                        set(&mut node.ann.min_items, n, &key, line)?
                    }
                    "maxItems" => {
                        let n = count(self)?;
                        set(&mut node.ann.max_items, n, &key, line)?
                    }
                    "pattern" => {
                        let p = self.quoted()?;
                        set(&mut node.ann.pattern, p, &key, line)?
                    }
                    "default" => {
                        let d = self.literal()?;
                        set(&mut node.ann.default, d, &key, line)?
                    }
                    other => return Err(err(line, format!("unknown annotation '{other}'"))),
                }
            } else if key == "closed" {
                match &mut node.kind {
                    Kind::Object(obj) if !obj.closed => obj.closed = true,
                    _ => return Err(err(line, "'closed' on a non-object or twice")),
                }
            } else {
                set_format(a, key, line)?;
            }
            match self.bump() {
                Some(',') => continue,
                Some(')') => return Ok(()),
                other => return Err(err(line, format!("expected ',' or ')', found {other:?}"))),
            }
        }
    }
}

fn set_format(a: &mut Ann, f: String, line: usize) -> Result<(), Error> {
    if a.format.is_some() {
        return Err(err(line, "two formats"));
    }
    a.format = Some(f);
    Ok(())
}

fn child_object_mut(node: &mut Node) -> Option<&mut Obj> {
    match &mut node.kind {
        Kind::Object(obj) => Some(obj),
        Kind::Array(Some(item)) => child_object_mut(item),
        _ => None,
    }
}

/// A field line split into its parts, before children are attached.
struct FieldLine {
    line: usize,
    depth: usize,
    prop: Prop,
}

fn parse_field(text: &str, line: usize) -> Result<FieldLine, Error> {
    let depth = text.chars().take_while(|c| *c == ' ').count();
    let body = &text[depth..];
    let colon = body
        .find(": ")
        .ok_or_else(|| err(line, "field line without ': '"))?;
    let (mut name, rest) = (&body[..colon], &body[colon + 2..]);
    let required = !name.ends_with('?');
    if !required {
        name = &name[..name.len() - 1];
    }
    if !is_valid_prop_name(name) {
        return Err(err(line, format!("bad field name '{name}'")));
    }
    let mut cur = Cursor::new(rest, line);
    let mut node = cur.parse_type()?;
    let mut tail = cur.rest();
    // An object (or array of objects) opens a `{ … }` block of child fields; nothing else may.
    let opens = tail.starts_with(" {");
    if opens {
        tail = tail[2..].to_string();
    }
    if opens != child_object_mut(&mut node).is_some() {
        return Err(err(line, "'{' must follow exactly the obj and obj[] types"));
    }
    if !tail.is_empty() {
        let d = tail
            .strip_prefix(" # ")
            .ok_or_else(|| err(line, format!("unexpected text after type: '{tail}'")))?;
        if d.is_empty() {
            return Err(err(line, "empty description"));
        }
        node.description = Some(d.to_string());
    }
    Ok(FieldLine {
        line,
        depth,
        prop: Prop {
            name: name.to_string(),
            required,
            node,
        },
    })
}

/// One indented line of a tool: a field, or the `}` closing an object's block.
enum Line {
    Field(Box<FieldLine>),
    Close { depth: usize },
    Taken,
}

/// Attach the run of fields at `depth` starting at `*i`, recursing into object blocks.
fn build(lines: &mut [Line], i: &mut usize, depth: usize) -> Result<Vec<Prop>, Error> {
    let mut props: Vec<Prop> = Vec::new();
    while *i < lines.len() {
        match &lines[*i] {
            // The caller owns its closing brace.
            Line::Close { .. } | Line::Taken => break,
            Line::Field(f) if f.depth < depth => break,
            Line::Field(f) if f.depth > depth => {
                return Err(err(f.line, "unexpected indentation"));
            }
            Line::Field(_) => {}
        }
        let Line::Field(mut f) = std::mem::replace(&mut lines[*i], Line::Taken) else {
            break;
        };
        *i += 1;
        if props.iter().any(|p| p.name == f.prop.name) {
            return Err(err(f.line, format!("duplicate field '{}'", f.prop.name)));
        }
        if let Some(obj) = child_object_mut(&mut f.prop.node) {
            obj.props = build(lines, i, depth + 1)?;
            match lines.get(*i) {
                Some(Line::Close { depth: d, .. }) if *d == depth => *i += 1,
                _ => return Err(err(f.line, "object block without its closing '}'")),
            }
        }
        props.push(f.prop);
    }
    Ok(props)
}

/// Parse a full definitions block.
pub(crate) fn parse_definitions(text: &str) -> Result<Vec<ParsedTool>, Error> {
    let mut tools: Vec<(usize, ParsedTool, Vec<Line>)> = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line = idx + 1;
        if raw.trim().is_empty() {
            continue;
        }
        if raw.starts_with(' ') {
            let parsed = if raw.trim_start() == "}" {
                Line::Close {
                    depth: raw.len() - 1,
                }
            } else {
                Line::Field(Box::new(parse_field(raw, line)?))
            };
            let Some((_, _, lines)) = tools.last_mut() else {
                return Err(err(line, "field before any tool header"));
            };
            lines.push(parsed);
            continue;
        }
        let colon = raw
            .find(':')
            .ok_or_else(|| err(line, "tool header without ':'"))?;
        let (mut name, rest) = (&raw[..colon], &raw[colon + 1..]);
        let closed = name.ends_with("(closed)");
        if closed {
            name = &name[..name.len() - "(closed)".len()];
        }
        if !crate::is_valid_tool_name(name) {
            return Err(err(line, format!("bad tool name '{name}'")));
        }
        let description = match rest.strip_prefix(' ') {
            Some(d) if !d.is_empty() => Some(d.to_string()),
            None if rest.is_empty() => None,
            _ => return Err(err(line, "malformed tool description")),
        };
        tools.push((
            line,
            ParsedTool {
                name: name.to_string(),
                description,
                params: Obj {
                    props: Vec::new(),
                    closed,
                },
            },
            Vec::new(),
        ));
    }
    let mut out = Vec::with_capacity(tools.len());
    for (line, mut tool, mut fields) in tools {
        let mut i = 0;
        tool.params.props = build(&mut fields, &mut i, 1)?;
        if i != fields.len() {
            return Err(err(line, "fields left over after parsing tool"));
        }
        out.push(tool);
    }
    Ok(out)
}
