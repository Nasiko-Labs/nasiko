//! The subset of JSON Schema the compact format expresses, and its three projections:
//! JSON Schema ⇄ [`Ty`] ⇄ compact signature text, plus validation of call arguments.
//!
//! Lowering is a **whitelist**: a schema keyword we do not model makes the tool
//! [`Error::Unsupported`], so compaction is bypassed rather than silently dropping meaning.

use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// String formats with a compact type name. Any other `format` is unsupported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    DateTime,
    Date,
    Time,
    Email,
    Uri,
}

impl Format {
    const ALL: [Format; 5] = [
        Format::DateTime,
        Format::Date,
        Format::Time,
        Format::Email,
        Format::Uri,
    ];

    fn json_name(self) -> &'static str {
        match self {
            Format::DateTime => "date-time",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
        }
    }

    fn compact_name(self) -> &'static str {
        match self {
            Format::DateTime => "datetime",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    Str(Option<Format>),
    Int,
    Num,
    Bool,
    /// `{"type":"object"}` with no `properties`: any JSON object.
    AnyObject,
    Array(Box<Ty>),
    Object(Vec<Field>),
    /// All strings or all integers (checked at lowering).
    Enum(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub name: String,
    pub ty: Ty,
    pub required: bool,
    pub description: Option<String>,
}

/// Words that are types, so a bare enum value may never be one of them.
const RESERVED: [&str; 13] = [
    "str", "int", "num", "bool", "obj", "datetime", "date", "time", "email", "uri", "true",
    "false", "null",
];

// ─── JSON Schema → Ty ────────────────────────────────────────────────────────────────────────

/// Lower a tool's `parameters` (root must be an object, or absent) to its field list.
pub(crate) fn lower_root(params: Option<&Value>) -> std::result::Result<Vec<Field>, String> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    let obj = params
        .as_object()
        .ok_or("parameters is not a JSON object")?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err("parameters root is not type object".into());
    }
    check_keys(obj, &["type", "properties", "required"])?;
    lower_fields(obj)
}

fn lower_fields(obj: &Map<String, Value>) -> std::result::Result<Vec<Field>, String> {
    let empty = Map::new();
    let props = match obj.get("properties") {
        None => &empty,
        Some(Value::Object(p)) => p,
        Some(_) => return Err("properties is not an object".into()),
    };
    let required: Vec<&str> = match obj.get("required") {
        None => Vec::new(),
        Some(Value::Array(r)) => r
            .iter()
            .map(|v| v.as_str().ok_or("required entry is not a string"))
            .collect::<std::result::Result<_, _>>()?,
        Some(_) => return Err("required is not an array".into()),
    };
    for r in &required {
        if !props.contains_key(*r) {
            return Err(format!("required field '{r}' has no property"));
        }
    }
    // Required fields first, in `required` order, then optional ones in map order. The map is
    // key-sorted (serde_json without `preserve_order`), so this is the only order we can keep —
    // and it makes `required` round-trip exactly through `decode_tools`.
    let mut order: Vec<(&String, &Value)> = props.iter().collect();
    order.sort_by_key(|(name, _)| {
        required
            .iter()
            .position(|r| r == name)
            .unwrap_or(usize::MAX)
    });
    order
        .into_iter()
        .map(|(name, schema)| {
            let schema = schema
                .as_object()
                .ok_or_else(|| format!("property '{name}' is not a schema object"))?;
            let description = match schema.get("description") {
                None => None,
                Some(Value::String(d)) => Some(d.clone()),
                Some(_) => return Err(format!("property '{name}' description is not a string")),
            };
            Ok(Field {
                name: name.clone(),
                ty: lower(schema, true).map_err(|e| format!("property '{name}': {e}"))?,
                required: required.contains(&name.as_str()),
                description,
            })
        })
        .collect()
}

/// Lower one schema. `described` says whether a `description` key is allowed here (it is
/// carried by the enclosing [`Field`]); elsewhere it would be dropped, so it is refused.
fn lower(obj: &Map<String, Value>, described: bool) -> std::result::Result<Ty, String> {
    let desc: &[&str] = if described { &["description"] } else { &[] };
    let allow = |keys: &[&str]| -> std::result::Result<(), String> {
        let all: Vec<&str> = keys.iter().chain(desc).copied().collect();
        check_keys(obj, &all)
    };

    let ty = match obj.get("type") {
        None => None,
        Some(Value::String(t)) => Some(t.as_str()),
        Some(_) => return Err("type must be a single string (unions/nullable unsupported)".into()),
    };

    if let Some(values) = obj.get("enum") {
        allow(&["type", "enum"])?;
        let values = values.as_array().ok_or("enum is not an array")?;
        if values.is_empty() {
            return Err("empty enum".into());
        }
        let all_str = values.iter().all(Value::is_string);
        let all_int = values.iter().all(|v| v.is_i64() || v.is_u64());
        match (ty, all_str, all_int) {
            (None | Some("string"), true, _) | (None | Some("integer"), _, true) => {}
            _ => return Err("enum must be all strings or all integers matching type".into()),
        }
        let mut seen = Vec::new();
        for v in values {
            if seen.contains(&v) {
                return Err("duplicate enum value".into());
            }
            seen.push(v);
        }
        return Ok(Ty::Enum(values.clone()));
    }

    match ty {
        Some("string") => {
            allow(&["type", "format"])?;
            match obj.get("format") {
                None => Ok(Ty::Str(None)),
                Some(Value::String(f)) => Format::ALL
                    .into_iter()
                    .find(|k| k.json_name() == f)
                    .map(|k| Ty::Str(Some(k)))
                    .ok_or_else(|| format!("string format '{f}' unsupported")),
                Some(_) => Err("format is not a string".into()),
            }
        }
        Some("integer") => allow(&["type"]).map(|()| Ty::Int),
        Some("number") => allow(&["type"]).map(|()| Ty::Num),
        Some("boolean") => allow(&["type"]).map(|()| Ty::Bool),
        Some("array") => {
            allow(&["type", "items"])?;
            let items = obj
                .get("items")
                .and_then(Value::as_object)
                .ok_or("array without an items schema")?;
            Ok(Ty::Array(Box::new(lower(items, false)?)))
        }
        Some("object") => {
            allow(&["type", "properties", "required"])?;
            if obj.contains_key("properties") {
                Ok(Ty::Object(lower_fields(obj)?))
            } else if obj.contains_key("required") {
                Err("required without properties".into())
            } else {
                Ok(Ty::AnyObject)
            }
        }
        Some(other) => Err(format!("type '{other}' unsupported")),
        None => Err("schema without type".into()),
    }
}

fn check_keys(obj: &Map<String, Value>, allowed: &[&str]) -> std::result::Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("keyword '{k}' unsupported")),
        None => Ok(()),
    }
}

// ─── Ty → JSON Schema (canonical) ────────────────────────────────────────────────────────────

pub(crate) fn raise_root(fields: &[Field]) -> Value {
    raise(&Ty::Object(fields.to_vec()))
}

fn raise(ty: &Ty) -> Value {
    let mut m = Map::new();
    let kind = |m: &mut Map<String, Value>, t: &str| {
        m.insert("type".into(), Value::String(t.into()));
    };
    match ty {
        Ty::Str(fmt) => {
            kind(&mut m, "string");
            if let Some(f) = fmt {
                m.insert("format".into(), Value::String(f.json_name().into()));
            }
        }
        Ty::Int => kind(&mut m, "integer"),
        Ty::Num => kind(&mut m, "number"),
        Ty::Bool => kind(&mut m, "boolean"),
        Ty::AnyObject => kind(&mut m, "object"),
        Ty::Array(items) => {
            kind(&mut m, "array");
            m.insert("items".into(), raise(items));
        }
        Ty::Object(fields) => {
            kind(&mut m, "object");
            let mut props = Map::new();
            for f in fields {
                let mut p = raise(&f.ty);
                if let (Some(d), Value::Object(po)) = (&f.description, &mut p) {
                    po.insert("description".into(), Value::String(d.clone()));
                }
                props.insert(f.name.clone(), p);
            }
            m.insert("properties".into(), Value::Object(props));
            let req: Vec<Value> = fields
                .iter()
                .filter(|f| f.required)
                .map(|f| Value::String(f.name.clone()))
                .collect();
            if !req.is_empty() {
                m.insert("required".into(), Value::Array(req));
            }
        }
        Ty::Enum(values) => {
            kind(
                &mut m,
                if values.iter().all(Value::is_string) {
                    "string"
                } else {
                    "integer"
                },
            );
            m.insert("enum".into(), Value::Array(values.clone()));
        }
    }
    Value::Object(m)
}

// ─── Ty → compact text ───────────────────────────────────────────────────────────────────────

pub(crate) fn render_fields(fields: &[Field], out: &mut String) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if is_plain_name(&f.name) {
            out.push_str(&f.name);
        } else {
            out.push_str(&quote(&f.name));
        }
        if !f.required {
            out.push('?');
        }
        out.push(':');
        render(&f.ty, out);
        if let Some(d) = &f.description {
            out.push(' ');
            out.push_str(&quote(d));
        }
    }
}

fn render(ty: &Ty, out: &mut String) {
    match ty {
        Ty::Str(None) => out.push_str("str"),
        Ty::Str(Some(f)) => out.push_str(f.compact_name()),
        Ty::Int => out.push_str("int"),
        Ty::Num => out.push_str("num"),
        Ty::Bool => out.push_str("bool"),
        Ty::AnyObject => out.push_str("obj"),
        Ty::Array(items) => {
            out.push('[');
            render(items, out);
            out.push(']');
        }
        Ty::Object(fields) => {
            out.push('{');
            render_fields(fields, out);
            out.push('}');
        }
        Ty::Enum(values) => {
            // A lone bare word would read as a type name, so single values are always quoted.
            let bare_ok = values.len() > 1;
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                match v {
                    Value::String(s) if bare_ok && is_bare_enum(s) => out.push_str(s),
                    Value::String(s) => out.push_str(&quote(s)),
                    other => out.push_str(&other.to_string()),
                }
            }
        }
    }
}

/// Single-quote a literal. Single quotes, unlike JSON's double quotes, need no escaping when the
/// prompt is itself embedded in a JSON request body — every `\"` there costs a token.
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
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn is_plain_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn is_bare_enum(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !RESERVED.contains(&s)
}

// ─── compact text → Ty ───────────────────────────────────────────────────────────────────────

/// Recursive-descent parser for the signature grammar (see the crate docs).
pub(crate) struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }

    pub(crate) fn rest(&self) -> &'a str {
        self.s.get(self.pos..).unwrap_or("")
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.pos).copied()
    }

    pub(crate) fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, c: u8) -> Result<()> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(self.err(&format!("expected '{}'", c as char)))
        }
    }

    pub(crate) fn err(&self, what: &str) -> Error {
        Error::InvalidCompact(format!("{what} at byte {} of {:?}", self.pos, self.s))
    }

    /// `[A-Za-z0-9_.-]+` — tool names.
    pub(crate) fn word(&mut self) -> Result<&'a str> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("expected a name"));
        }
        Ok(self.s.get(start..self.pos).unwrap_or(""))
    }

    /// Inverse of [`quote`].
    pub(crate) fn quoted(&mut self) -> Result<String> {
        self.expect(b'\'')?;
        let mut out = String::new();
        let mut chars = self.rest().char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\'' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next().map(|(_, e)| e) {
                    Some('\\') => out.push('\\'),
                    Some('\'') => out.push('\''),
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    _ => return Err(self.err("bad escape")),
                },
                c => out.push(c),
            }
        }
        Err(self.err("unterminated string"))
    }

    pub(crate) fn fields(&mut self, close: u8) -> Result<Vec<Field>> {
        let mut fields = Vec::new();
        if self.eat(close) {
            return Ok(fields);
        }
        loop {
            let name = if self.peek() == Some(b'\'') {
                self.quoted()?
            } else {
                let start = self.pos;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
                {
                    self.pos += 1;
                }
                if self.pos == start {
                    return Err(self.err("expected a field name"));
                }
                self.s.get(start..self.pos).unwrap_or("").to_owned()
            };
            let required = !self.eat(b'?');
            self.expect(b':')?;
            let ty = self.ty()?;
            let description = if self.rest().starts_with(" '") {
                self.pos += 1;
                Some(self.quoted()?)
            } else {
                None
            };
            if fields.iter().any(|f: &Field| f.name == name) {
                return Err(self.err("duplicate field"));
            }
            fields.push(Field {
                name,
                ty,
                required,
                description,
            });
            if self.eat(close) {
                return Ok(fields);
            }
            self.expect(b',')?;
            self.eat(b' ');
        }
    }

    fn ty(&mut self) -> Result<Ty> {
        match self.peek() {
            Some(b'[') => {
                self.pos += 1;
                let inner = self.ty()?;
                self.expect(b']')?;
                Ok(Ty::Array(Box::new(inner)))
            }
            Some(b'{') => {
                self.pos += 1;
                Ok(Ty::Object(self.fields(b'}')?))
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let save = self.pos;
                let w = self.enum_word();
                if self.peek() != Some(b'|') {
                    let prim = match w {
                        "str" => Some(Ty::Str(None)),
                        "int" => Some(Ty::Int),
                        "num" => Some(Ty::Num),
                        "bool" => Some(Ty::Bool),
                        "obj" => Some(Ty::AnyObject),
                        _ => Format::ALL
                            .into_iter()
                            .find(|f| f.compact_name() == w)
                            .map(|f| Ty::Str(Some(f))),
                    };
                    return prim.ok_or_else(|| self.err("unknown type"));
                }
                self.pos = save;
                self.enum_values()
            }
            Some(_) => self.enum_values(),
            None => Err(self.err("expected a type")),
        }
    }

    fn enum_word(&mut self) -> &'a str {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            self.pos += 1;
        }
        self.s.get(start..self.pos).unwrap_or("")
    }

    fn enum_values(&mut self) -> Result<Ty> {
        let mut values = Vec::new();
        loop {
            let v = match self.peek() {
                Some(b'\'') => Value::String(self.quoted()?),
                Some(c) if c == b'-' || c.is_ascii_digit() => {
                    let start = self.pos;
                    self.pos += 1;
                    while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                        self.pos += 1;
                    }
                    let lit = self.s.get(start..self.pos).unwrap_or("");
                    let n: i64 = lit.parse().map_err(|_| self.err("bad integer"))?;
                    Value::from(n)
                }
                Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                    let w = self.enum_word();
                    if RESERVED.contains(&w) {
                        return Err(self.err("reserved word used as enum value"));
                    }
                    Value::String(w.to_owned())
                }
                _ => return Err(self.err("expected an enum value")),
            };
            values.push(v);
            if !self.eat(b'|') {
                break;
            }
        }
        let all_str = values.iter().all(Value::is_string);
        let all_int = values.iter().all(Value::is_i64);
        if !(all_str || all_int) {
            return Err(self.err("mixed enum"));
        }
        Ok(Ty::Enum(values))
    }
}

// ─── validation ──────────────────────────────────────────────────────────────────────────────

/// Validate call arguments against a tool's root fields. Unknown keys are rejected: a
/// hallucinated argument is a wrong call, and dropping it would silently alter the call.
pub(crate) fn validate_root(args: &Value, fields: &[Field]) -> std::result::Result<(), String> {
    validate(args, &Ty::Object(fields.to_vec()), "$")
}

fn validate(v: &Value, ty: &Ty, path: &str) -> std::result::Result<(), String> {
    let mismatch = |want: &str| Err(format!("{path}: expected {want}, got {}", kind_of(v)));
    match ty {
        Ty::Str(fmt) => {
            let Some(s) = v.as_str() else {
                return mismatch("string");
            };
            match fmt {
                Some(f) if !format_ok(*f, s) => {
                    Err(format!("{path}: {s:?} is not a valid {}", f.json_name()))
                }
                _ => Ok(()),
            }
        }
        Ty::Int => {
            let integral = v.is_i64()
                || v.is_u64()
                || v.as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0);
            if integral {
                Ok(())
            } else {
                mismatch("integer")
            }
        }
        Ty::Num => {
            if v.is_number() {
                Ok(())
            } else {
                mismatch("number")
            }
        }
        Ty::Bool => {
            if v.is_boolean() {
                Ok(())
            } else {
                mismatch("boolean")
            }
        }
        Ty::AnyObject => {
            if v.is_object() {
                Ok(())
            } else {
                mismatch("object")
            }
        }
        Ty::Array(items) => {
            let Some(arr) = v.as_array() else {
                return mismatch("array");
            };
            arr.iter()
                .enumerate()
                .try_for_each(|(i, x)| validate(x, items, &format!("{path}[{i}]")))
        }
        Ty::Enum(values) => {
            if values.contains(v) {
                Ok(())
            } else {
                Err(format!(
                    "{path}: {v} is not one of {}",
                    Value::from(values.clone())
                ))
            }
        }
        Ty::Object(fields) => {
            let Some(obj) = v.as_object() else {
                return mismatch("object");
            };
            if let Some(k) = obj.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
                return Err(format!("{path}: unknown field '{k}'"));
            }
            for f in fields {
                match obj.get(&f.name) {
                    Some(x) => validate(x, &f.ty, &format!("{path}.{}", f.name))?,
                    None if f.required => {
                        return Err(format!("{path}: missing required field '{}'", f.name));
                    }
                    None => {}
                }
            }
            Ok(())
        }
    }
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn format_ok(f: Format, s: &str) -> bool {
    match f {
        Format::DateTime => {
            let Some(i) = s.find(['T', 't', ' ']) else {
                return false;
            };
            let (d, t) = s.split_at(i);
            is_date(d) && is_time(t.get(1..).unwrap_or(""), true)
        }
        Format::Date => is_date(s),
        Format::Time => is_time(s, false),
        Format::Email => {
            let mut parts = s.split('@');
            matches!((parts.next(), parts.next(), parts.next()), (Some(l), Some(d), None)
                if !l.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.'))
                && !s.chars().any(char::is_whitespace)
        }
        Format::Uri => s.split_once(':').is_some_and(|(scheme, rest)| {
            !rest.is_empty()
                && scheme
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
                && !s.chars().any(char::is_whitespace)
        }),
    }
}

/// `n` ASCII digits parsed as a number, or `None`.
fn digits(s: &str, n: usize) -> Option<u32> {
    (s.len() == n && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn is_date(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    let [y, m, d] = p.as_slice() else {
        return false;
    };
    let (Some(y), Some(m), Some(d)) = (digits(y, 4), digits(m, 2), digits(d, 2)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max).contains(&d)
}

/// `HH:MM:SS[.frac]` then, if `offset_required`, `Z` or `±HH:MM` (optional otherwise).
fn is_time(s: &str, offset_required: bool) -> bool {
    let (clock, offset) = match s.find(['Z', 'z', '+', '-']) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    let (hms, frac) = clock.split_once('.').unwrap_or((clock, "0"));
    let p: Vec<&str> = hms.split(':').collect();
    let [h, m, sec] = p.as_slice() else {
        return false;
    };
    let ok_clock = matches!((digits(h, 2), digits(m, 2), digits(sec, 2)),
        (Some(h), Some(m), Some(s)) if h < 24 && m < 60 && s <= 60)
        && !frac.is_empty()
        && frac.bytes().all(|b| b.is_ascii_digit());
    let ok_offset = match offset {
        "" => !offset_required,
        "Z" | "z" => true,
        o => o.get(1..).and_then(|hm| hm.split_once(':')).is_some_and(
            |(h, m)| matches!((digits(h, 2), digits(m, 2)), (Some(h), Some(m)) if h < 24 && m < 60),
        ),
    };
    ok_clock && ok_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn date_time_formats() {
        assert!(format_ok(Format::DateTime, "2026-10-05T15:00:00+05:30"));
        assert!(format_ok(Format::DateTime, "2026-10-05T15:00:00.123Z"));
        assert!(!format_ok(Format::DateTime, "2026-10-05T15:00:00"));
        assert!(!format_ok(Format::DateTime, "2026-02-30T15:00:00Z"));
        assert!(!format_ok(Format::DateTime, "Monday 3pm"));
        assert!(format_ok(Format::Date, "2024-02-29"));
        assert!(!format_ok(Format::Date, "2026-02-29"));
        assert!(format_ok(Format::Time, "10:00:00"));
        assert!(format_ok(Format::Email, "riya@example.com"));
        assert!(!format_ok(Format::Email, "riya at example"));
        assert!(format_ok(Format::Uri, "https://nasiko.dev/x"));
        assert!(!format_ok(Format::Uri, "not a uri"));
    }

    #[test]
    fn single_value_and_reserved_enums_are_quoted() {
        let mut out = String::new();
        render(&Ty::Enum(vec![json!("public")]), &mut out);
        assert_eq!(out, "'public'");
        out.clear();
        render(
            &Ty::Enum(vec![json!("str"), json!("a b"), json!("ok")]),
            &mut out,
        );
        assert_eq!(out, "'str'|'a b'|ok");
    }

    #[test]
    fn integer_accepts_integral_float_but_not_fraction() {
        assert!(validate(&json!(30.0), &Ty::Int, "$").is_ok());
        assert!(validate(&json!(30.5), &Ty::Int, "$").is_err());
        assert!(validate(&json!("30"), &Ty::Int, "$").is_err());
    }
}
