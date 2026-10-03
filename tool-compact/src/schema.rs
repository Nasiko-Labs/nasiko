//! Compact schema lines. The grammar is `GRAMMAR.md`.

use serde_json::{Map, Value};

use crate::{CompactTools, Error, Result, ToolDef};

const LINE_EMIT: &str = "Emit <<call name {json}>>.";
const LINE_EXAMPLE: &str = r#"Example: <<call do_thing {"k":"v"}>>"#;

#[derive(Debug)]
enum Ty {
    Str,
    Int,
    Number,
    Bool,
    DateTime,
    Array(Box<Ty>),
    Enum(Vec<String>),
    Object(Vec<Field>, Extra),
}

#[derive(Debug)]
struct Field {
    name: String,
    optional: bool,
    ty: Ty,
}

/// Extra keys on an object. Absent and `false` are both [`Extra::Reject`].
#[derive(Debug)]
enum Extra {
    Reject,
    AllowAny,
    Schema(Box<Ty>),
}

pub(crate) fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let mut lines = Vec::with_capacity(tools.len() + 2);
    for tool in tools {
        lines.push(encode_one(tool)?);
    }
    lines.push(LINE_EMIT.to_string());
    lines.push(LINE_EXAMPLE.to_string());
    Ok(CompactTools {
        text: lines.join("\n"),
    })
}

pub(crate) fn decode_tools(text: &str) -> Result<Vec<ToolDef>> {
    if text.is_empty() {
        return Err(Error::InvalidCompact);
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let Some((example, rest)) = lines.split_last() else {
        return Err(Error::InvalidCompact);
    };
    let Some((emit, tool_lines)) = rest.split_last() else {
        return Err(Error::InvalidCompact);
    };
    if *emit != LINE_EMIT || *example != LINE_EXAMPLE {
        return Err(Error::InvalidCompact);
    }
    let mut tools = Vec::with_capacity(tool_lines.len());
    for line in tool_lines {
        if line.is_empty() {
            return Err(Error::InvalidCompact);
        }
        tools.push(parse_tool_line(line)?);
    }
    Ok(tools)
}

fn encode_one(tool: &ToolDef) -> Result<String> {
    let mut out = String::new();
    write_token(&mut out, &tool.name)?;
    if let Some(params) = &tool.parameters {
        let Ty::Object(fields, extra) = schema_to_ty(params)? else {
            return Err(Error::Unsupported);
        };
        out.push('(');
        write_fields(&mut out, &fields, &extra)?;
        out.push(')');
    }
    if let Some(desc) = tool.description.as_deref().and_then(normalize_desc) {
        out.push_str(" - ");
        push_escaped_desc(&mut out, &desc);
    }
    Ok(out)
}

fn normalize_desc(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

fn push_escaped_desc(out: &mut String, text: &str) {
    for ch in text.chars() {
        if ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
}

fn write_fields(out: &mut String, fields: &[Field], extra: &Extra) -> Result<()> {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_token(out, &field.name)?;
        if field.optional {
            out.push('?');
        }
        out.push(':');
        write_ty(out, &field.ty)?;
    }
    match extra {
        Extra::Reject => {}
        Extra::AllowAny => {
            if !fields.is_empty() {
                out.push_str(", ");
            }
            out.push_str("...");
        }
        Extra::Schema(ty) => {
            if !fields.is_empty() {
                out.push_str(", ");
            }
            out.push_str("...:");
            write_ty(out, ty)?;
        }
    }
    Ok(())
}

fn write_ty(out: &mut String, ty: &Ty) -> Result<()> {
    match ty {
        Ty::Str => out.push_str("str"),
        Ty::Int => out.push_str("int"),
        Ty::Number => out.push_str("number"),
        Ty::Bool => out.push_str("bool"),
        Ty::DateTime => out.push_str("datetime"),
        Ty::Array(inner) => {
            out.push('[');
            write_ty(out, inner)?;
            out.push(']');
        }
        Ty::Enum(vals) => write_enum(out, vals)?,
        Ty::Object(fields, extra) => {
            out.push('{');
            write_fields(out, fields, extra)?;
            out.push('}');
        }
    }
    Ok(())
}

fn write_enum(out: &mut String, vals: &[String]) -> Result<()> {
    let alone = vals.len() == 1;
    for (i, val) in vals.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        if is_bare(val) && !(alone && is_keyword(val)) {
            out.push_str(val);
        } else {
            out.push_str(&json_string(val)?);
        }
    }
    Ok(())
}

fn write_token(out: &mut String, text: &str) -> Result<()> {
    if is_bare(text) {
        out.push_str(text);
    } else {
        out.push_str(&json_string(text)?);
    }
    Ok(())
}

fn json_string(text: &str) -> Result<String> {
    serde_json::to_string(text).map_err(|_| Error::InvalidCompact)
}

fn is_keyword(text: &str) -> bool {
    matches!(text, "str" | "int" | "number" | "bool" | "datetime")
}

fn is_bare(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(ch) if is_ident_start(ch) => chars.all(is_ident_cont),
        _ => false,
    }
}

pub(crate) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

pub(crate) fn is_ident_cont(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn schema_to_ty(value: &Value) -> Result<Ty> {
    let Some(obj) = value.as_object() else {
        return Err(Error::Unsupported);
    };
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "type"
                | "properties"
                | "required"
                | "items"
                | "enum"
                | "format"
                | "description"
                | "additionalProperties"
        ) {
            return Err(Error::Unsupported);
        }
    }
    let ty_name = match obj.get("type") {
        None => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => return Err(Error::Unsupported),
    };
    let format = match obj.get("format") {
        None => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => return Err(Error::Unsupported),
    };
    if let Some(enum_v) = obj.get("enum") {
        return enum_ty(obj, ty_name, format, enum_v);
    }
    let ty = match ty_name {
        Some("string") => string_ty(obj, format)?,
        Some("integer") => scalar_ty(obj, format, Ty::Int)?,
        Some("number") => scalar_ty(obj, format, Ty::Number)?,
        Some("boolean") => scalar_ty(obj, format, Ty::Bool)?,
        Some("array") => array_ty(obj, format)?,
        Some("object") => object_ty(obj)?,
        None if obj.contains_key("properties") || obj.contains_key("additionalProperties") => {
            object_ty(obj)?
        }
        _ => return Err(Error::Unsupported),
    };
    if obj.contains_key("additionalProperties") && !matches!(ty, Ty::Object(_, _)) {
        return Err(Error::Unsupported);
    }
    Ok(ty)
}

fn enum_ty(
    obj: &Map<String, Value>,
    ty_name: Option<&str>,
    format: Option<&str>,
    enum_v: &Value,
) -> Result<Ty> {
    if format.is_some()
        || obj.contains_key("properties")
        || obj.contains_key("items")
        || obj.contains_key("required")
        || obj.contains_key("additionalProperties")
        || (ty_name.is_some() && ty_name != Some("string"))
    {
        return Err(Error::Unsupported);
    }
    let Some(items) = enum_v.as_array() else {
        return Err(Error::Unsupported);
    };
    if items.is_empty() {
        return Err(Error::Unsupported);
    }
    let mut vals = Vec::with_capacity(items.len());
    for item in items {
        let Some(text) = item.as_str() else {
            return Err(Error::Unsupported);
        };
        vals.push(text.to_string());
    }
    Ok(Ty::Enum(vals))
}

fn string_ty(obj: &Map<String, Value>, format: Option<&str>) -> Result<Ty> {
    if has_structure(obj) {
        return Err(Error::Unsupported);
    }
    match format {
        Some("date-time") => Ok(Ty::DateTime),
        None => Ok(Ty::Str),
        Some(_) => Err(Error::Unsupported),
    }
}

fn scalar_ty(obj: &Map<String, Value>, format: Option<&str>, ty: Ty) -> Result<Ty> {
    if format.is_some() || has_structure(obj) {
        return Err(Error::Unsupported);
    }
    Ok(ty)
}

fn array_ty(obj: &Map<String, Value>, format: Option<&str>) -> Result<Ty> {
    if format.is_some() || obj.contains_key("properties") || obj.contains_key("required") {
        return Err(Error::Unsupported);
    }
    let Some(items) = obj.get("items") else {
        return Err(Error::Unsupported);
    };
    if items.is_array() {
        return Err(Error::Unsupported);
    }
    Ok(Ty::Array(Box::new(schema_to_ty(items)?)))
}

fn object_ty(obj: &Map<String, Value>) -> Result<Ty> {
    if obj.contains_key("items") || obj.contains_key("enum") || obj.contains_key("format") {
        return Err(Error::Unsupported);
    }
    let props = match obj.get("properties") {
        None => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => return Err(Error::Unsupported),
    };
    let mut required = Vec::new();
    if let Some(req) = obj.get("required") {
        let Some(items) = req.as_array() else {
            return Err(Error::Unsupported);
        };
        for item in items {
            let Some(name) = item.as_str() else {
                return Err(Error::Unsupported);
            };
            if required.iter().any(|have: &String| have == name) {
                return Err(Error::Unsupported);
            }
            required.push(name.to_string());
        }
    }
    let mut fields = Vec::new();
    if let Some(props) = props {
        for (name, schema) in props {
            let optional = !required.iter().any(|have| have == name);
            fields.push(Field {
                name: name.clone(),
                optional,
                ty: schema_to_ty(schema)?,
            });
        }
    }
    for name in &required {
        if !fields.iter().any(|field| &field.name == name) {
            return Err(Error::Unsupported);
        }
    }
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    let extra = match obj.get("additionalProperties") {
        None => Extra::Reject,
        Some(value) => extra_of(value)?,
    };
    Ok(Ty::Object(fields, extra))
}

fn extra_of(value: &Value) -> Result<Extra> {
    match value {
        Value::Bool(false) => Ok(Extra::Reject),
        Value::Bool(true) => Ok(Extra::AllowAny),
        Value::Object(_) => Ok(Extra::Schema(Box::new(schema_to_ty(value)?))),
        _ => Err(Error::Unsupported),
    }
}

fn has_structure(obj: &Map<String, Value>) -> bool {
    obj.contains_key("properties") || obj.contains_key("items") || obj.contains_key("required")
}

fn ty_to_schema(ty: &Ty) -> Value {
    match ty {
        Ty::Str => json_type("string"),
        Ty::Int => json_type("integer"),
        Ty::Number => json_type("number"),
        Ty::Bool => json_type("boolean"),
        Ty::DateTime => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("string".to_string()));
            obj.insert("format".to_string(), Value::String("date-time".to_string()));
            Value::Object(obj)
        }
        Ty::Array(inner) => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("array".to_string()));
            obj.insert("items".to_string(), ty_to_schema(inner));
            Value::Object(obj)
        }
        Ty::Enum(vals) => {
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("string".to_string()));
            obj.insert(
                "enum".to_string(),
                Value::Array(vals.iter().cloned().map(Value::String).collect()),
            );
            Value::Object(obj)
        }
        Ty::Object(fields, extra) => {
            let mut props = Map::new();
            let mut required = Vec::new();
            for field in fields {
                if !field.optional {
                    required.push(Value::String(field.name.clone()));
                }
                props.insert(field.name.clone(), ty_to_schema(&field.ty));
            }
            let mut obj = Map::new();
            obj.insert("type".to_string(), Value::String("object".to_string()));
            obj.insert("properties".to_string(), Value::Object(props));
            if !required.is_empty() {
                obj.insert("required".to_string(), Value::Array(required));
            }
            match extra {
                Extra::Reject => {}
                Extra::AllowAny => {
                    obj.insert("additionalProperties".to_string(), Value::Bool(true));
                }
                Extra::Schema(ty) => {
                    obj.insert("additionalProperties".to_string(), ty_to_schema(ty));
                }
            }
            Value::Object(obj)
        }
    }
}

fn json_type(name: &str) -> Value {
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String(name.to_string()));
    Value::Object(obj)
}

struct Cur {
    chars: Vec<char>,
    i: usize,
}

impl Cur {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            i: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.i += 1;
        Some(ch)
    }

    fn rest(&self) -> String {
        self.chars[self.i..].iter().collect()
    }
}

fn parse_tool_line(line: &str) -> Result<ToolDef> {
    let mut cur = Cur::new(line);
    let name = read_token(&mut cur)?;
    let parameters = if cur.peek() == Some('(') {
        cur.bump();
        let (fields, extra) = read_fields(&mut cur, ')')?;
        Some(ty_to_schema(&Ty::Object(fields, extra)))
    } else {
        None
    };
    let description = take_desc(&mut cur)?;
    if cur.peek().is_some() {
        return Err(Error::InvalidCompact);
    }
    Ok(ToolDef {
        name,
        description,
        parameters,
    })
}

fn take_desc(cur: &mut Cur) -> Result<Option<String>> {
    if cur.peek().is_none() {
        return Ok(None);
    }
    if cur.bump() != Some(' ') || cur.bump() != Some('-') || cur.bump() != Some(' ') {
        return Err(Error::InvalidCompact);
    }
    let rest = cur.rest();
    cur.i = cur.chars.len();
    let desc = unescape_desc(&rest)?;
    if desc.is_empty() {
        return Err(Error::InvalidCompact);
    }
    Ok(Some(desc))
}

fn unescape_desc(text: &str) -> Result<String> {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            _ => return Err(Error::InvalidCompact),
        }
    }
    Ok(out)
}

fn read_fields(cur: &mut Cur, end: char) -> Result<(Vec<Field>, Extra)> {
    if cur.peek() == Some(end) {
        cur.bump();
        return Ok((Vec::new(), Extra::Reject));
    }
    let mut fields = Vec::new();
    loop {
        if cur.peek() == Some('.') {
            let extra = read_extra(cur)?;
            if cur.bump() != Some(end) {
                return Err(Error::InvalidCompact);
            }
            check_dupes(&fields)?;
            return Ok((fields, extra));
        }
        fields.push(read_field(cur)?);
        match cur.bump() {
            Some(',') => {
                if cur.bump() != Some(' ') {
                    return Err(Error::InvalidCompact);
                }
            }
            Some(ch) if ch == end => {
                check_dupes(&fields)?;
                return Ok((fields, Extra::Reject));
            }
            _ => return Err(Error::InvalidCompact),
        }
    }
}

fn read_extra(cur: &mut Cur) -> Result<Extra> {
    for _ in 0..3 {
        if cur.bump() != Some('.') {
            return Err(Error::InvalidCompact);
        }
    }
    if cur.peek() == Some(':') {
        cur.bump();
        return Ok(Extra::Schema(Box::new(read_ty(cur)?)));
    }
    Ok(Extra::AllowAny)
}

fn check_dupes(fields: &[Field]) -> Result<()> {
    for (i, field) in fields.iter().enumerate() {
        if fields.iter().take(i).any(|prev| prev.name == field.name) {
            return Err(Error::InvalidCompact);
        }
    }
    Ok(())
}

fn read_field(cur: &mut Cur) -> Result<Field> {
    let name = read_token(cur)?;
    let optional = if cur.peek() == Some('?') {
        cur.bump();
        true
    } else {
        false
    };
    if cur.bump() != Some(':') {
        return Err(Error::InvalidCompact);
    }
    Ok(Field {
        name,
        optional,
        ty: read_ty(cur)?,
    })
}

fn read_ty(cur: &mut Cur) -> Result<Ty> {
    match cur.peek() {
        Some('[') => {
            cur.bump();
            let inner = read_ty(cur)?;
            if cur.bump() != Some(']') {
                return Err(Error::InvalidCompact);
            }
            Ok(Ty::Array(Box::new(inner)))
        }
        Some('{') => {
            cur.bump();
            let (fields, extra) = read_fields(cur, '}')?;
            Ok(Ty::Object(fields, extra))
        }
        Some('"') => {
            let first = read_json_string(cur)?;
            read_enum_rest(cur, first)
        }
        Some(ch) if is_ident_start(ch) => {
            let word = read_bare(cur)?;
            if let Some(ty) = keyword_ty(&word)
                && cur.peek() != Some('|')
            {
                return Ok(ty);
            }
            read_enum_rest(cur, word)
        }
        _ => Err(Error::InvalidCompact),
    }
}

fn keyword_ty(word: &str) -> Option<Ty> {
    Some(match word {
        "str" => Ty::Str,
        "int" => Ty::Int,
        "number" => Ty::Number,
        "bool" => Ty::Bool,
        "datetime" => Ty::DateTime,
        _ => return None,
    })
}

fn read_enum_rest(cur: &mut Cur, first: String) -> Result<Ty> {
    let mut vals = vec![first];
    while cur.peek() == Some('|') {
        cur.bump();
        vals.push(read_enum_atom(cur)?);
    }
    Ok(Ty::Enum(vals))
}

fn read_enum_atom(cur: &mut Cur) -> Result<String> {
    if cur.peek() == Some('"') {
        read_json_string(cur)
    } else {
        read_bare(cur)
    }
}

fn read_token(cur: &mut Cur) -> Result<String> {
    if cur.peek() == Some('"') {
        read_json_string(cur)
    } else {
        read_bare(cur)
    }
}

fn read_bare(cur: &mut Cur) -> Result<String> {
    match cur.peek() {
        Some(ch) if is_ident_start(ch) => {}
        _ => return Err(Error::InvalidCompact),
    }
    let mut out = String::new();
    while let Some(ch) = cur.peek() {
        if !is_ident_cont(ch) {
            break;
        }
        out.push(ch);
        cur.bump();
    }
    Ok(out)
}

fn read_json_string(cur: &mut Cur) -> Result<String> {
    let start = cur.i;
    if cur.bump() != Some('"') {
        return Err(Error::InvalidCompact);
    }
    loop {
        match cur.bump() {
            Some('\\') => {
                if cur.bump().is_none() {
                    return Err(Error::InvalidCompact);
                }
            }
            Some('"') => break,
            Some(_) => {}
            None => return Err(Error::InvalidCompact),
        }
    }
    let raw: String = cur.chars[start..cur.i].iter().collect();
    serde_json::from_str(&raw).map_err(|_| Error::InvalidCompact)
}

pub(crate) fn check_arguments(parameters: &Option<Value>, value: &Value) -> Result<()> {
    let Some(obj) = value.as_object() else {
        return Err(Error::InvalidArguments);
    };
    let Some(schema) = parameters else {
        return if obj.is_empty() {
            Ok(())
        } else {
            Err(Error::InvalidArguments)
        };
    };
    let Ty::Object(fields, extra) = schema_to_ty(schema)? else {
        return Err(Error::Unsupported);
    };
    check_fields(obj, &fields, &extra)
}

fn check_fields(obj: &Map<String, Value>, fields: &[Field], extra: &Extra) -> Result<()> {
    for (key, value) in obj {
        if fields.iter().any(|field| &field.name == key) {
            continue;
        }
        match extra {
            Extra::Reject => return Err(Error::InvalidArguments),
            Extra::AllowAny => {}
            Extra::Schema(ty) => check_ty(value, ty)?,
        }
    }
    for field in fields {
        match obj.get(&field.name) {
            None if field.optional => {}
            None => return Err(Error::InvalidArguments),
            Some(value) => check_ty(value, &field.ty)?,
        }
    }
    Ok(())
}

fn check_ty(value: &Value, ty: &Ty) -> Result<()> {
    match ty {
        Ty::Str => value.as_str().map(|_| ()).ok_or(Error::InvalidArguments),
        Ty::Int => {
            if value.as_i64().is_some() || value.as_u64().is_some() {
                Ok(())
            } else {
                Err(Error::InvalidArguments)
            }
        }
        Ty::Number => value.as_number().map(|_| ()).ok_or(Error::InvalidArguments),
        Ty::Bool => value.as_bool().map(|_| ()).ok_or(Error::InvalidArguments),
        Ty::DateTime => match value.as_str() {
            Some(text) if is_datetime(text) => Ok(()),
            _ => Err(Error::InvalidArguments),
        },
        Ty::Enum(vals) => match value.as_str() {
            Some(text) if vals.iter().any(|v| v == text) => Ok(()),
            _ => Err(Error::InvalidArguments),
        },
        Ty::Array(inner) => {
            let Some(items) = value.as_array() else {
                return Err(Error::InvalidArguments);
            };
            for item in items {
                check_ty(item, inner)?;
            }
            Ok(())
        }
        Ty::Object(fields, extra) => {
            let Some(obj) = value.as_object() else {
                return Err(Error::InvalidArguments);
            };
            check_fields(obj, fields, extra)
        }
    }
}

fn is_datetime(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() < 20 || !text.is_ascii() {
        return false;
    }
    if !(digits(b, 0, 4)
        && b[4] == b'-'
        && digits(b, 5, 2)
        && b[7] == b'-'
        && digits(b, 8, 2)
        && (b[10] == b'T' || b[10] == b't')
        && digits(b, 11, 2)
        && b[13] == b':'
        && digits(b, 14, 2)
        && b[16] == b':'
        && digits(b, 17, 2))
    {
        return false;
    }
    if !in_range(b, 5, 1, 12)
        || !in_range(b, 8, 1, 31)
        || !in_range(b, 11, 0, 23)
        || !in_range(b, 14, 0, 59)
        || !in_range(b, 17, 0, 60)
    {
        return false;
    }
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    match b.get(i) {
        Some(b'Z' | b'z') => i + 1 == b.len(),
        Some(b'+' | b'-') => {
            i + 6 == b.len()
                && digits(b, i + 1, 2)
                && b.get(i + 3) == Some(&b':')
                && digits(b, i + 4, 2)
                && in_range(b, i + 1, 0, 23)
                && in_range(b, i + 4, 0, 59)
        }
        _ => false,
    }
}

fn digits(bytes: &[u8], at: usize, n: usize) -> bool {
    bytes
        .get(at..at.saturating_add(n))
        .is_some_and(|slice| slice.len() == n && slice.iter().all(u8::is_ascii_digit))
}

fn in_range(bytes: &[u8], at: usize, lo: u16, hi: u16) -> bool {
    let Some(slice) = bytes.get(at..at + 2) else {
        return false;
    };
    if !slice.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let value = u16::from(slice[0] - b'0') * 10 + u16::from(slice[1] - b'0');
    (lo..=hi).contains(&value)
}
