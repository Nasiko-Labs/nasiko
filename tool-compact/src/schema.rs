//! The supported JSON Schema subset, as a small typed IR.
//!
//! Every tool schema is lowered into [`Field`]/[`Ty`] before anything is rendered. Lowering is
//! where unsupported features are detected, so the compact renderer, the compact parser and the
//! argument validator all work from one representation and cannot disagree about what a schema
//! means. Anything the IR cannot express is rejected here, and the caller bypasses compaction.

use serde_json::{Map, Value};

/// Keywords that annotate a schema without changing which values it accepts. They are dropped.
const ANNOTATIONS: &[&str] = &["title", "$schema"];

/// Bare type names in the compact grammar. A string enum value spelled like one of these is
/// always quoted, so `str|int` can never be read as a two-value enum by mistake.
const RESERVED: &[&str] = &[
    "str", "int", "num", "bool", "null", "true", "false", "datetime", "date", "time", "email",
    "uri", "uuid",
];

/// String formats with a compact alias. Any other `format` is unsupported, because dropping it
/// would silently weaken the schema the model sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    DateTime,
    Date,
    Time,
    Email,
    Uri,
    Uuid,
}

impl Format {
    const ALL: [Format; 6] = [
        Format::DateTime,
        Format::Date,
        Format::Time,
        Format::Email,
        Format::Uri,
        Format::Uuid,
    ];

    fn json_name(self) -> &'static str {
        match self {
            Format::DateTime => "date-time",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
            Format::Uuid => "uuid",
        }
    }

    fn alias(self) -> &'static str {
        match self {
            Format::DateTime => "datetime",
            Format::Date => "date",
            Format::Time => "time",
            Format::Email => "email",
            Format::Uri => "uri",
            Format::Uuid => "uuid",
        }
    }

    fn from_json_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.json_name() == s)
    }

    fn from_alias(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.alias() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    /// `format` is kept as a hint to the model. Like JSON Schema itself, validation treats it as
    /// an annotation: any string is accepted.
    Str(Option<Format>),
    Int,
    Num,
    Bool,
    Array(Box<Ty>),
    Object(Vec<Field>),
    /// All-string or all-integer values only.
    Enum(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub name: String,
    pub required: bool,
    pub ty: Ty,
    pub default: Option<Value>,
    /// Whitespace-normalized; never empty.
    pub desc: Option<String>,
}

// ── JSON Schema → IR ─────────────────────────────────────────────────────────────────────────

/// Lower a tool's `parameters`. `None`, `{}` and an object with no `properties` all mean
/// "takes no arguments".
pub(crate) fn params_from_json(params: Option<&Value>) -> Result<Vec<Field>, String> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    let obj = params
        .as_object()
        .ok_or_else(|| "parameters is not a JSON object".to_string())?;
    if obj.is_empty() {
        return Ok(Vec::new());
    }
    check_keys(
        obj,
        &["type", "properties", "required", "additionalProperties"],
        "",
    )?;
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err("root schema must have type \"object\"".into());
    }
    if !obj.contains_key("properties") {
        if obj.contains_key("required") {
            return Err("`required` without `properties`".into());
        }
        return Ok(Vec::new());
    }
    object_fields(obj, "")
}

fn object_fields(obj: &Map<String, Value>, path: &str) -> Result<Vec<Field>, String> {
    if let Some(extra) = obj.get("additionalProperties")
        && extra != &Value::Bool(false)
    {
        return Err(format!("{}: open `additionalProperties`", at(path)));
    }
    let props = obj
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{}: `properties` is not an object", at(path)))?;

    let mut required: Vec<&str> = Vec::new();
    match obj.get("required") {
        None => {}
        Some(Value::Array(names)) => {
            for name in names {
                let name = name
                    .as_str()
                    .ok_or_else(|| format!("{}: non-string in `required`", at(path)))?;
                if !props.contains_key(name) {
                    return Err(format!("{}: required `{name}` is not a property", at(path)));
                }
                if !required.contains(&name) {
                    required.push(name);
                }
            }
        }
        Some(_) => return Err(format!("{}: `required` is not an array", at(path))),
    }

    // Required fields first, in the author's `required` order, then optional ones by name. The
    // order is deterministic without depending on JSON map ordering.
    let mut fields = Vec::with_capacity(props.len());
    for name in &required {
        if let Some(schema) = props.get(*name) {
            fields.push(field(name, schema, true, path)?);
        }
    }
    for (name, schema) in props {
        if !required.contains(&name.as_str()) {
            fields.push(field(name, schema, false, path)?);
        }
    }
    Ok(fields)
}

fn field(name: &str, schema: &Value, required: bool, path: &str) -> Result<Field, String> {
    let path = format!("{path}{name}");
    if !is_ident(name) {
        return Err(format!("property name `{name}` is not a plain identifier"));
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema is not an object"))?;
    let desc = match obj.get("description") {
        None => None,
        Some(Value::String(s)) => normalize_ws(s),
        Some(_) => return Err(format!("{path}: non-string description")),
    };
    let ty = ty_from(obj, &["description", "default"], &path)?;
    let default = obj.get("default").cloned();
    if let Some(d) = &default {
        validate(&ty, d, &path).map_err(|e| format!("default does not fit schema: {e}"))?;
    }
    Ok(Field {
        name: name.to_string(),
        required,
        ty,
        default,
        desc,
    })
}

fn ty_from(obj: &Map<String, Value>, field_keys: &[&str], path: &str) -> Result<Ty, String> {
    let allow = |own: &[&str]| -> Result<(), String> {
        let all: Vec<&str> = own.iter().chain(field_keys).copied().collect();
        check_keys(obj, &all, path)
    };

    if let Some(values) = obj.get("enum") {
        allow(&["enum", "type"])?;
        let values = values
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{path}: `enum` must be a non-empty array"))?;
        let declared = obj.get("type").and_then(Value::as_str);
        let all_str = values.iter().all(Value::is_string);
        let all_int = values.iter().all(|v| v.is_i64() || v.is_u64());
        match (declared, all_str, all_int) {
            (None | Some("string"), true, _) | (None | Some("integer"), _, true) => {}
            _ => {
                return Err(format!(
                    "{path}: only all-string or all-integer enums are supported"
                ));
            }
        }
        return Ok(Ty::Enum(values.clone()));
    }

    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{path}: `type` missing or not a single string"))?;
    match kind {
        "string" => {
            allow(&["type", "format"])?;
            let format = match obj.get("format") {
                None => None,
                Some(Value::String(f)) => Some(
                    Format::from_json_name(f)
                        .ok_or_else(|| format!("{path}: unsupported string format `{f}`"))?,
                ),
                Some(_) => return Err(format!("{path}: non-string `format`")),
            };
            Ok(Ty::Str(format))
        }
        "integer" => allow(&["type"]).map(|()| Ty::Int),
        "number" => allow(&["type"]).map(|()| Ty::Num),
        "boolean" => allow(&["type"]).map(|()| Ty::Bool),
        "array" => {
            allow(&["type", "items"])?;
            let items = obj
                .get("items")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("{path}: array without an `items` schema"))?;
            Ok(Ty::Array(Box::new(ty_from(
                items,
                &[],
                &format!("{path}[]"),
            )?)))
        }
        "object" => {
            allow(&["type", "properties", "required", "additionalProperties"])?;
            if !obj.contains_key("properties") {
                return Err(format!("{path}: free-form object (no `properties`)"));
            }
            Ok(Ty::Object(object_fields(obj, &format!("{path}."))?))
        }
        other => Err(format!("{path}: unsupported type `{other}`")),
    }
}

fn check_keys(obj: &Map<String, Value>, allowed: &[&str], path: &str) -> Result<(), String> {
    match obj
        .keys()
        .find(|k| !allowed.contains(&k.as_str()) && !ANNOTATIONS.contains(&k.as_str()))
    {
        Some(k) => Err(format!("{}: unsupported keyword `{k}`", at(path))),
        None => Ok(()),
    }
}

fn at(path: &str) -> &str {
    if path.is_empty() { "parameters" } else { path }
}

// ── IR → JSON Schema ─────────────────────────────────────────────────────────────────────────

pub(crate) fn params_to_json(fields: &[Field]) -> Value {
    Value::Object(object_json(fields))
}

fn object_json(fields: &[Field]) -> Map<String, Value> {
    let mut props = Map::new();
    for f in fields {
        let mut schema = ty_json(&f.ty);
        if let Some(d) = &f.desc {
            schema.insert("description".into(), Value::String(d.clone()));
        }
        if let Some(d) = &f.default {
            schema.insert("default".into(), d.clone());
        }
        props.insert(f.name.clone(), Value::Object(schema));
    }
    let mut out = Map::new();
    out.insert("type".into(), "object".into());
    out.insert("properties".into(), Value::Object(props));
    let required: Vec<Value> = fields
        .iter()
        .filter(|f| f.required)
        .map(|f| Value::String(f.name.clone()))
        .collect();
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    out
}

fn ty_json(ty: &Ty) -> Map<String, Value> {
    let mut m = Map::new();
    match ty {
        Ty::Str(format) => {
            m.insert("type".into(), "string".into());
            if let Some(f) = format {
                m.insert("format".into(), f.json_name().into());
            }
        }
        Ty::Int => {
            m.insert("type".into(), "integer".into());
        }
        Ty::Num => {
            m.insert("type".into(), "number".into());
        }
        Ty::Bool => {
            m.insert("type".into(), "boolean".into());
        }
        Ty::Array(items) => {
            m.insert("type".into(), "array".into());
            m.insert("items".into(), Value::Object(ty_json(items)));
        }
        Ty::Object(fields) => m = object_json(fields),
        Ty::Enum(values) => {
            let kind = if values.iter().all(Value::is_string) {
                "string"
            } else {
                "integer"
            };
            m.insert("type".into(), kind.into());
            m.insert("enum".into(), Value::Array(values.clone()));
        }
    }
    m
}

// ── IR → compact text ────────────────────────────────────────────────────────────────────────

pub(crate) fn render_fields(fields: &[Field], out: &mut String) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&f.name);
        if !f.required {
            out.push('?');
        }
        out.push(':');
        render_ty(&f.ty, out);
        if let Some(d) = &f.default {
            out.push('=');
            out.push_str(&d.to_string());
        }
        if let Some(d) = &f.desc {
            out.push(' ');
            out.push_str(&Value::String(d.clone()).to_string());
        }
    }
}

fn render_ty(ty: &Ty, out: &mut String) {
    match ty {
        Ty::Str(None) => out.push_str("str"),
        Ty::Str(Some(f)) => out.push_str(f.alias()),
        Ty::Int => out.push_str("int"),
        Ty::Num => out.push_str("num"),
        Ty::Bool => out.push_str("bool"),
        Ty::Array(items) => {
            out.push('[');
            render_ty(items, out);
            out.push(']');
        }
        Ty::Object(fields) => {
            out.push('{');
            render_fields(fields, out);
            out.push('}');
        }
        Ty::Enum(values) => {
            let single = values.len() == 1;
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                match v {
                    Value::String(s) if !single && is_bare_literal(s) => out.push_str(s),
                    other => out.push_str(&other.to_string()),
                }
            }
        }
    }
}

// ── compact text → IR ────────────────────────────────────────────────────────────────────────

/// A cursor over one compact definition line.
pub(crate) struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        Self { s, i: 0 }
    }

    pub(crate) fn rest(&self) -> &'a str {
        self.s.get(self.i..).unwrap_or("")
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    pub(crate) fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += c.len_utf8();
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, c: char) -> Result<(), String> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}` at byte {} in {:?}", self.i, self.s))
        }
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|c| c == ' ') {
            self.i += 1;
        }
    }

    pub(crate) fn ident(&mut self) -> Result<&'a str, String> {
        let rest = self.rest();
        let len = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        if len == 0 {
            return Err(format!(
                "expected a name at byte {} in {:?}",
                self.i, self.s
            ));
        }
        self.i += len;
        Ok(rest.get(..len).unwrap_or(""))
    }

    fn json_value(&mut self) -> Result<Value, String> {
        let rest = self.rest();
        let len = json_span(rest).ok_or_else(|| format!("bad JSON literal at byte {}", self.i))?;
        let span = rest.get(..len).unwrap_or("");
        let v =
            serde_json::from_str(span).map_err(|e| format!("bad JSON literal {span:?}: {e}"))?;
        self.i += len;
        Ok(v)
    }

    /// `field ("," field)*`, up to (not consuming) `close`.
    pub(crate) fn fields(&mut self, close: char) -> Result<Vec<Field>, String> {
        let mut out = Vec::new();
        self.skip_ws();
        if self.peek() == Some(close) {
            return Ok(out);
        }
        loop {
            out.push(self.field()?);
            self.skip_ws();
            if !self.eat(',') {
                return Ok(out);
            }
            self.skip_ws();
        }
    }

    fn field(&mut self) -> Result<Field, String> {
        let name = self.ident()?.to_string();
        let required = !self.eat('?');
        self.expect(':')?;
        let ty = self.ty()?;
        let default = if self.eat('=') {
            Some(self.json_value()?)
        } else {
            None
        };
        self.skip_ws();
        let desc = if self.peek() == Some('"') {
            match self.json_value()? {
                Value::String(s) => Some(s),
                _ => return Err("description is not a string".into()),
            }
        } else {
            None
        };
        Ok(Field {
            name,
            required,
            ty,
            default,
            desc,
        })
    }

    fn ty(&mut self) -> Result<Ty, String> {
        if self.eat('[') {
            let items = self.ty()?;
            self.expect(']')?;
            return Ok(Ty::Array(Box::new(items)));
        }
        if self.eat('{') {
            let fields = self.fields('}')?;
            self.expect('}')?;
            return Ok(Ty::Object(fields));
        }
        match self.enum_atom()? {
            Atom::Type(t) if self.peek() != Some('|') => Ok(t),
            Atom::Type(_) => Err("a type keyword cannot be an enum member".into()),
            Atom::Lit(v) => {
                let mut values = vec![v];
                while self.eat('|') {
                    match self.enum_atom()? {
                        Atom::Lit(v) => values.push(v),
                        Atom::Type(_) => {
                            return Err("a type keyword cannot be an enum member".into());
                        }
                    }
                }
                Ok(Ty::Enum(values))
            }
        }
    }

    fn enum_atom(&mut self) -> Result<Atom, String> {
        match self.peek() {
            Some('"') => Ok(Atom::Lit(self.json_value()?)),
            Some(c) if c == '-' || c.is_ascii_digit() => Ok(Atom::Lit(self.json_value()?)),
            _ => {
                let word = self.ident()?;
                let ty = match word {
                    "str" => Some(Ty::Str(None)),
                    "int" => Some(Ty::Int),
                    "num" => Some(Ty::Num),
                    "bool" => Some(Ty::Bool),
                    w => Format::from_alias(w).map(|f| Ty::Str(Some(f))),
                };
                Ok(match ty {
                    Some(t) => Atom::Type(t),
                    None => Atom::Lit(Value::String(word.to_string())),
                })
            }
        }
    }
}

enum Atom {
    Type(Ty),
    Lit(Value),
}

/// Byte length of the JSON value at the start of `s`: a string, an array/object (bracket- and
/// string-aware), or a scalar running to the next delimiter.
fn json_span(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    match bytes.first()? {
        b'"' => {
            let mut escaped = false;
            for (i, &b) in bytes.iter().enumerate().skip(1) {
                match (escaped, b) {
                    (true, _) => escaped = false,
                    (false, b'\\') => escaped = true,
                    (false, b'"') => return Some(i + 1),
                    _ => {}
                }
            }
            None
        }
        b'[' | b'{' => crate::decode::balanced_len(s),
        _ => {
            let len = s
                .find([',', ')', '}', ']', '|', '=', ' '])
                .unwrap_or(s.len());
            (len > 0).then_some(len)
        }
    }
}

// ── validation ───────────────────────────────────────────────────────────────────────────────

pub(crate) fn validate(ty: &Ty, v: &Value, path: &str) -> Result<(), String> {
    let ok = match ty {
        Ty::Str(_) => v.is_string(),
        Ty::Int => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        Ty::Num => v.is_number(),
        Ty::Bool => v.is_boolean(),
        Ty::Array(items) => {
            let arr = v
                .as_array()
                .ok_or_else(|| format!("`{path}` must be an array"))?;
            for (i, item) in arr.iter().enumerate() {
                validate(items, item, &format!("{path}[{i}]"))?;
            }
            true
        }
        Ty::Object(fields) => {
            let obj = v
                .as_object()
                .ok_or_else(|| format!("`{path}` must be an object"))?;
            return validate_object(fields, obj, &format!("{path}."));
        }
        Ty::Enum(values) => {
            if values.iter().any(|allowed| json_eq(allowed, v)) {
                true
            } else {
                return Err(format!("`{path}` = {v} is not one of the allowed values"));
            }
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!("`{path}` has the wrong type ({v})"))
    }
}

pub(crate) fn validate_object(
    fields: &[Field],
    obj: &Map<String, Value>,
    prefix: &str,
) -> Result<(), String> {
    if let Some(unknown) = obj.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
        return Err(format!("unknown argument `{prefix}{unknown}`"));
    }
    for f in fields {
        match obj.get(&f.name) {
            Some(v) => validate(&f.ty, v, &format!("{prefix}{}", f.name))?,
            None if f.required => {
                return Err(format!("missing required argument `{prefix}{}`", f.name));
            }
            None => {}
        }
    }
    Ok(())
}

/// Equality that treats `1` and `1.0` as the same number.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

// ── lexical helpers ──────────────────────────────────────────────────────────────────────────

pub(crate) fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

pub(crate) fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(is_ident_char)
}

fn is_bare_literal(s: &str) -> bool {
    s.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && is_ident(s)
        && !RESERVED.contains(&s)
}

/// Collapse all whitespace runs to one space; `None` when nothing is left.
pub(crate) fn normalize_ws(s: &str) -> Option<String> {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}
