//! The supported JSON-Schema subset as a small typed model.
//!
//! One model drives all four jobs, so they cannot drift apart: [`compile`] (schema → model, or a
//! bypass reason), [`Sig::render`] (model → compact text), [`parse_line`] (compact text → model)
//! and [`validate_args`] (model × call arguments → accept/reject).
//!
//! The rule is **representable exactly or not at all**: any keyword the model cannot carry makes
//! [`compile`] fail, and the caller then keeps the original schema untouched.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::ToolDef;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
    Str,
    Int,
    Num,
    Bool,
    DateTime,
    Enum(Vec<String>),
    Array(Box<Ty>),
    Object(Vec<Field>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    name: String,
    required: bool,
    ty: Ty,
    desc: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sig {
    name: String,
    desc: Option<String>,
    fields: Vec<Field>,
}

const RESERVED: [&str; 5] = ["str", "int", "num", "bool", "datetime"];

pub(crate) fn tool_name_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(tool_name_byte)
}

pub(crate) fn tool_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn field_name_ok(s: &str) -> bool {
    let mut it = s.chars();
    it.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && it.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn enum_token_ok(s: &str) -> bool {
    !s.is_empty()
        && !RESERVED.contains(&s)
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.+/@#-".contains(c))
}

// ── schema → model ──────────────────────────────────────────────────────────

/// Compile a tool into the compact model, or say why it has no exact compact form.
pub(crate) fn compile(tool: &ToolDef) -> Result<Sig, String> {
    if !tool_name_ok(&tool.name) {
        return Err(format!("tool name {:?} is not representable", tool.name));
    }
    if let Some(d) = &tool.description
        && (d.is_empty() || d != d.trim() || d.chars().any(char::is_control))
    {
        return Err("description is empty, padded or multi-line".into());
    }
    let fields = match &tool.parameters {
        None => vec![],
        Some(p) => {
            let m = p.as_object().ok_or("parameters is not an object")?;
            if m.get("type").and_then(Value::as_str) != Some("object") {
                return Err("top-level parameters is not `type: object`".into());
            }
            object_fields(m, false)?
        }
    };
    Ok(Sig {
        name: tool.name.clone(),
        desc: tool.description.clone(),
        fields,
    })
}

fn only_keys(m: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("keyword `{k}` has no compact form")),
        None => Ok(()),
    }
}

fn object_fields(m: &Map<String, Value>, in_prop: bool) -> Result<Vec<Field>, String> {
    only_keys(
        m,
        &[
            "type",
            "properties",
            "required",
            "additionalProperties",
            "description",
        ][..if in_prop { 5 } else { 4 }],
    )?;
    match m.get("additionalProperties") {
        None | Some(Value::Bool(false)) => {}
        Some(_) => {
            return Err(
                "open objects (`additionalProperties` other than false) are unsupported".into(),
            );
        }
    }
    let props = m
        .get("properties")
        .and_then(Value::as_object)
        .ok_or("object without `properties` is open-ended")?;
    let required: Vec<&str> = match m.get("required") {
        None => vec![],
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().ok_or("`required` entry is not a string"))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("`required` is not an array".into()),
    };
    let mut seen = std::collections::HashSet::new();
    for r in &required {
        if !props.contains_key(*r) {
            return Err(format!("`required` names undeclared property `{r}`"));
        }
        if !seen.insert(*r) {
            return Err(format!("`required` lists `{r}` twice"));
        }
    }
    // Required fields keep the schema's own `required` order (so it reconstructs exactly);
    // optional ones are sorted so output never depends on map iteration order.
    let mut optional: Vec<&String> = props
        .keys()
        .filter(|k| !seen.contains(k.as_str()))
        .collect();
    optional.sort();
    let order = required
        .iter()
        .map(|r| (*r, true))
        .chain(optional.into_iter().map(|k| (k.as_str(), false)));
    order
        .map(|(name, req)| {
            if !field_name_ok(name) {
                return Err(format!("property name {name:?} is not representable"));
            }
            let (ty, desc) = prop(&props[name])?;
            Ok(Field {
                name: name.to_string(),
                required: req,
                ty,
                desc,
            })
        })
        .collect()
}

fn prop(node: &Value) -> Result<(Ty, Option<String>), String> {
    let m = node.as_object().ok_or("property schema is not an object")?;
    let desc = match m.get("description") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("`description` is not a string".into()),
    };
    Ok((ty_of(m, true)?, desc))
}

fn ty_of(m: &Map<String, Value>, in_prop: bool) -> Result<Ty, String> {
    let desc: &[&str] = if in_prop { &["description"] } else { &[] };
    let allow = |extra: &[&str]| only_keys(m, &[extra, desc].concat());
    let kind = match m.get("type") {
        None => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err("`type` must be a single string".into()),
    };
    if m.contains_key("enum") {
        if kind.is_some_and(|k| k != "string") {
            return Err("only string enums are supported".into());
        }
        allow(&["type", "enum"])?;
        let vals = m["enum"].as_array().ok_or("`enum` is not an array")?;
        let vals: Vec<String> = vals
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or("non-string enum value")
            })
            .collect::<Result<_, _>>()?;
        let unique: std::collections::HashSet<_> = vals.iter().collect();
        if vals.len() < 2 || unique.len() != vals.len() {
            return Err("enum needs at least two distinct values".into());
        }
        if let Some(bad) = vals.iter().find(|v| !enum_token_ok(v)) {
            return Err(format!("enum value {bad:?} is not representable"));
        }
        return Ok(Ty::Enum(vals));
    }
    match kind {
        Some("string") => {
            allow(&["type", "format"])?;
            match m.get("format") {
                None => Ok(Ty::Str),
                Some(Value::String(f)) if f == "date-time" => Ok(Ty::DateTime),
                Some(f) => Err(format!("format {f} is not enforced by the compact form")),
            }
        }
        Some("integer") => allow(&["type"]).map(|()| Ty::Int),
        Some("number") => allow(&["type"]).map(|()| Ty::Num),
        Some("boolean") => allow(&["type"]).map(|()| Ty::Bool),
        Some("array") => {
            allow(&["type", "items"])?;
            let items = m
                .get("items")
                .and_then(Value::as_object)
                .ok_or("array without a single `items` schema")?;
            Ok(Ty::Array(Box::new(ty_of(items, false)?)))
        }
        Some("object") => object_fields(m, in_prop).map(Ty::Object),
        Some(other) => Err(format!("type `{other}` has no compact form")),
        None => Err("schema without `type`".into()),
    }
}

// ── model → compact text ────────────────────────────────────────────────────

impl Sig {
    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.name);
        out.push('(');
        render_fields(&self.fields, &mut out);
        out.push(')');
        if let Some(d) = &self.desc {
            out.push_str(" - ");
            out.push_str(d);
        }
        out
    }

    pub(crate) fn into_tool(self) -> ToolDef {
        let parameters = (!self.fields.is_empty()).then(|| object_schema(&self.fields));
        ToolDef {
            name: self.name,
            description: self.desc,
            parameters,
        }
    }
}

fn render_fields(fields: &[Field], out: &mut String) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&f.name);
        if !f.required {
            out.push('?');
        }
        out.push(':');
        render_ty(&f.ty, out);
        if let Some(d) = &f.desc {
            out.push(' ');
            // A JSON string literal: delimiters, quotes and newlines in prose cannot break the line.
            out.push_str(&Value::String(d.clone()).to_string());
        }
    }
}

fn render_ty(ty: &Ty, out: &mut String) {
    match ty {
        Ty::Str => out.push_str("str"),
        Ty::Int => out.push_str("int"),
        Ty::Num => out.push_str("num"),
        Ty::Bool => out.push_str("bool"),
        Ty::DateTime => out.push_str("datetime"),
        Ty::Enum(v) => out.push_str(&v.join("|")),
        Ty::Array(t) => {
            out.push('[');
            render_ty(t, out);
            out.push(']');
        }
        Ty::Object(f) => {
            out.push('{');
            render_fields(f, out);
            out.push('}');
        }
    }
}

// ── model → schema ──────────────────────────────────────────────────────────

fn object_schema(fields: &[Field]) -> Value {
    let props: BTreeMap<&str, Value> = fields
        .iter()
        .map(|f| (f.name.as_str(), node_schema(f)))
        .collect();
    let required: Vec<&str> = fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.name.as_str())
        .collect();
    let mut s = json!({"type": "object", "properties": props});
    if !required.is_empty() {
        s["required"] = json!(required);
    }
    s
}

fn node_schema(f: &Field) -> Value {
    let mut s = ty_schema(&f.ty);
    if let Some(d) = &f.desc {
        s["description"] = Value::String(d.clone());
    }
    s
}

fn ty_schema(ty: &Ty) -> Value {
    match ty {
        Ty::Str => json!({"type": "string"}),
        Ty::Int => json!({"type": "integer"}),
        Ty::Num => json!({"type": "number"}),
        Ty::Bool => json!({"type": "boolean"}),
        Ty::DateTime => json!({"type": "string", "format": "date-time"}),
        Ty::Enum(v) => json!({"type": "string", "enum": v}),
        Ty::Array(t) => json!({"type": "array", "items": ty_schema(t)}),
        Ty::Object(f) => object_schema(f),
    }
}

// ── compact text → model (for `decode_tools`) ───────────────────────────────

pub(crate) fn parse_line(line: &str) -> Result<Sig, String> {
    let mut p = Parser { s: line, i: 0 };
    let name = p.take_while(|c| c.is_ascii() && tool_name_byte(c as u8));
    if !tool_name_ok(name) {
        return Err(format!("bad tool name in {line:?}"));
    }
    let name = name.to_string();
    p.expect('(')?;
    let fields = p.fields(')')?;
    p.expect(')')?;
    let rest = &line[p.i..];
    let desc = match rest {
        "" => None,
        r => Some(
            r.strip_prefix(" - ")
                .ok_or("expected ` - ` before description")?
                .to_string(),
        ),
    };
    Ok(Sig { name, desc, fields })
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.s[self.i..].chars().next()
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> &'a str {
        let start = self.i;
        while let Some(c) = self.peek().filter(|c| f(*c)) {
            self.i += c.len_utf8();
        }
        &self.s[start..self.i]
    }

    fn expect(&mut self, want: char) -> Result<(), String> {
        match self.peek() {
            Some(c) if c == want => {
                self.i += c.len_utf8();
                Ok(())
            }
            other => Err(format!(
                "expected `{want}` at byte {}, found {other:?}",
                self.i
            )),
        }
    }

    fn fields(&mut self, close: char) -> Result<Vec<Field>, String> {
        let mut out = vec![];
        while self.peek() != Some(close) {
            if !out.is_empty() {
                self.expect(',')?;
            }
            let name = self.take_while(|c| c.is_ascii_alphanumeric() || c == '_');
            if !field_name_ok(name) {
                return Err(format!("bad property name {name:?}"));
            }
            let name = name.to_string();
            let required = self.peek() != Some('?');
            if !required {
                self.expect('?')?;
            }
            self.expect(':')?;
            let ty = self.ty()?;
            let desc = if self.s[self.i..].starts_with(" \"") {
                self.i += 1;
                Some(self.string_literal()?)
            } else {
                None
            };
            out.push(Field {
                name,
                required,
                ty,
                desc,
            });
        }
        Ok(out)
    }

    fn ty(&mut self) -> Result<Ty, String> {
        match self.peek() {
            Some('[') => {
                self.expect('[')?;
                let t = self.ty()?;
                self.expect(']')?;
                Ok(Ty::Array(Box::new(t)))
            }
            Some('{') => {
                self.expect('{')?;
                let f = self.fields('}')?;
                self.expect('}')?;
                Ok(Ty::Object(f))
            }
            _ => {
                let tok = self.take_while(|c| c.is_ascii_alphanumeric() || "_.+/@#-|".contains(c));
                match tok {
                    "str" => Ok(Ty::Str),
                    "int" => Ok(Ty::Int),
                    "num" => Ok(Ty::Num),
                    "bool" => Ok(Ty::Bool),
                    "datetime" => Ok(Ty::DateTime),
                    _ => {
                        let vals: Vec<String> = tok.split('|').map(str::to_string).collect();
                        if vals.len() < 2 || !vals.iter().all(|v| enum_token_ok(v)) {
                            return Err(format!("bad type {tok:?}"));
                        }
                        Ok(Ty::Enum(vals))
                    }
                }
            }
        }
    }

    /// Parse one JSON string literal starting at the current `"`.
    fn string_literal(&mut self) -> Result<String, String> {
        let rest = &self.s[self.i..];
        let mut escaped = false;
        for (off, c) in rest.char_indices().skip(1) {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => {
                    let lit = &rest[..=off];
                    self.i += lit.len();
                    return serde_json::from_str(lit)
                        .map_err(|e| format!("bad description literal: {e}"));
                }
                _ => {}
            }
        }
        Err("unterminated description literal".into())
    }
}

// ── validation ──────────────────────────────────────────────────────────────

/// Check call arguments against the compiled schema. `Err` carries a path-prefixed reason.
pub(crate) fn validate_args(sig: &Sig, args: &Value) -> Result<(), String> {
    check_object(&sig.fields, args, "$")
}

fn check_object(fields: &[Field], v: &Value, path: &str) -> Result<(), String> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("{path}: expected object"))?;
    if let Some(k) = m.keys().find(|k| !fields.iter().any(|f| &f.name == *k)) {
        return Err(format!("{path}: unknown argument `{k}`"));
    }
    for f in fields {
        match m.get(&f.name) {
            Some(x) => check(&f.ty, x, &format!("{path}.{}", f.name))?,
            None if f.required => {
                return Err(format!("{path}: missing required argument `{}`", f.name));
            }
            None => {}
        }
    }
    Ok(())
}

fn check(ty: &Ty, v: &Value, path: &str) -> Result<(), String> {
    let bad = |want: &str| Err(format!("{path}: expected {want}"));
    match (ty, v) {
        (Ty::Str, Value::String(_)) | (Ty::Bool, Value::Bool(_)) | (Ty::Num, Value::Number(_)) => {
            Ok(())
        }
        (Ty::Int, Value::Number(n)) if n.is_i64() || n.is_u64() => Ok(()),
        (Ty::Int, _) => bad("integer"),
        (Ty::DateTime, Value::String(s)) if is_rfc3339(s) => Ok(()),
        (Ty::DateTime, _) => bad("RFC 3339 date-time with offset"),
        (Ty::Enum(vals), Value::String(s)) if vals.contains(s) => Ok(()),
        (Ty::Enum(vals), _) => bad(&format!("one of {}", vals.join("|"))),
        (Ty::Array(t), Value::Array(items)) => items
            .iter()
            .enumerate()
            .try_for_each(|(i, x)| check(t, x, &format!("{path}[{i}]"))),
        (Ty::Array(_), _) => bad("array"),
        (Ty::Object(f), v) => check_object(f, v, path),
        (Ty::Str, _) => bad("string"),
        (Ty::Bool, _) => bad("boolean"),
        (Ty::Num, _) => bad("number"),
    }
}

/// RFC 3339 `date-time`: `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`; the offset is mandatory.
fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    let num = |from: usize, len: usize| -> Option<u32> {
        let d = b.get(from..from + len)?;
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| d.iter().fold(0, |a, c| a * 10 + u32::from(c - b'0')))
    };
    let sep = |at: usize, ok: &[u8]| b.get(at).is_some_and(|c| ok.contains(c));
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        num(0, 4),
        num(5, 2),
        num(8, 2),
        num(11, 2),
        num(14, 2),
        num(17, 2),
    ) else {
        return false;
    };
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt") && sep(13, b":") && sep(16, b":")) {
        return false;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let dim = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if d == 0 || d > dim || h > 23 || mi > 59 || se > 60 {
        return false;
    }
    let mut i = 19;
    if sep(i, b".") {
        let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return false;
        }
        i += 1 + digits;
    }
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            i + 6 == b.len()
                && sep(i + 3, b":")
                && num(i + 1, 2).is_some_and(|h| h <= 23)
                && num(i + 4, 2).is_some_and(|m| m <= 59)
        }
        _ => false,
    }
}
