//! The A2 compact form: rendering, parsing it back (`decode_tools`) and the encode-time
//! self-check.
//!
//! ```text
//! create_event # Create a calendar event
//!  start:datetime
//!  title:str
//!  attendees?:[email]
//!  limit?:int=10
//!  options?:{}!
//!   visibility:public|private|"on hold"
//! get_time()
//! Call tools as <<call NAME {JSON}>> (no XML/tags)
//! ```
//!
//! * Tool line: `NAME` then `()` (no parameters) or `!` (closed top-level object), then an
//!   optional ` # DESCRIPTION`.
//! * Field line: one space of indent per nesting level, `NAME`, `?` if optional, `:TYPE`,
//!   optional `=DEFAULT` (a JSON literal), optional ` # DESCRIPTION`. Required fields come
//!   first, then optional, each alphabetical.
//! * TYPE: `str int num bool datetime date email uri`, `[TYPE]` (array), `{}` (object; its
//!   fields follow one level deeper), `{}!` (object with `additionalProperties: false`),
//!   `a|b|c` (string enum; a literal that is not a plain word is written as a JSON string)
//!   or `1|2|3` (integer enum).
//! * Descriptions carry only what the line does not already say: spaces are squeezed,
//!   trailing periods and a leading "Optional" on optional fields are dropped, and a
//!   description whose every word is already given by the names on the line, the type, or
//!   filler (`the`, `of`, `user`, ...) is left out.
//!
//! Types, required-ness, enums and defaults are never changed; the self-check proves it on
//! every encode. Anything this cannot state exactly is `Error::Unsupported` and the caller
//! sends native tools.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::schema::{Format, Kind, Node, Object, normalize_tool, unsupported};
use crate::{Error, ToolDef};

/// Last line of every compact block: how the model calls a tool.
pub const INSTRUCTION: &str = "Call tools as <<call NAME {JSON}>> (no XML/tags)";

/// Characters that are grammar syntax and so cannot appear in a bare enum literal.
const SYNTAX: &[char] = &[
    '|', '#', ':', '?', '!', '[', ']', '{', '}', '(', ')', '=', '"',
];

/// Words that say nothing on their own in a description.
const FILLER: &[&str] = &[
    "a", "an", "and", "be", "by", "for", "from", "in", "is", "it", "its", "of", "on", "or", "s",
    "that", "the", "this", "to", "user", "with",
];

/// Tool definitions rendered in the compact form, ready to go in the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

/// One tool as the model sees it: the parsed schema with descriptions trimmed.
struct Shown {
    name: String,
    description: Option<String>,
    params: Option<Node>,
}

/// Render `tools` compactly. Fails with `Error::Unsupported` if any tool cannot be stated
/// exactly, or if the result does not decode back to the same schemas (self-check).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    if tools.is_empty() {
        return Err(unsupported("", "no tools"));
    }
    let mut seen = BTreeSet::new();
    let mut shown = Vec::new();
    let mut text = String::new();
    for tool in tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(unsupported(&tool.name, "duplicate tool name"));
        }
        let s = show(tool)?;
        render_tool(&s, &mut text)?;
        shown.push(s);
    }
    text.push_str(INSTRUCTION);
    let compact = CompactTools { text };
    self_check(tools, &shown, &compact)?;
    Ok(compact)
}

/// Parse a compact block back into tool definitions with canonical JSON Schemas.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, Error> {
    let lines: Vec<&str> = compact.text.split('\n').collect();
    let body = match lines.as_slice() {
        [body @ .., last] if *last == INSTRUCTION => body,
        _ => return Err(malformed("missing instruction line")),
    };
    let mut tools = Vec::new();
    let mut i = 0;
    while let Some(line) = body.get(i) {
        i += 1;
        let (head, description) = split_description(line);
        let (name, params) = if let Some(name) = head.strip_suffix("()") {
            (name, None)
        } else {
            let (name, closed) = match head.strip_suffix('!') {
                Some(name) => (name, true),
                None => (head, false),
            };
            let mut obj = empty_object(closed);
            parse_fields(body, &mut i, 1, &mut obj)?;
            let node = Node {
                kind: Kind::Object(obj),
                description: None,
                default: None,
            };
            (name, Some(node.to_json()))
        };
        if !is_name(name) {
            return Err(malformed(format!("bad tool line `{line}`")));
        }
        tools.push(ToolDef {
            name: name.to_string(),
            description,
            parameters: params,
        });
    }
    Ok(tools)
}

/// The form two equivalent schemas share: `required` sorted, empty `properties` and
/// `required` dropped (they mean the same as absent).
pub fn canonical_schema(v: &Value) -> Value {
    let Value::Object(map) = v else {
        return v.clone();
    };
    let mut out = Map::new();
    for (k, val) in map {
        let val = match (k.as_str(), val) {
            ("properties", Value::Object(props)) if props.is_empty() => continue,
            ("properties", Value::Object(props)) => Value::Object(
                props
                    .iter()
                    .map(|(name, s)| (name.clone(), canonical_schema(s)))
                    .collect(),
            ),
            ("required", Value::Array(names)) if names.is_empty() => continue,
            ("required", Value::Array(names)) => {
                let mut names = names.clone();
                names.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                Value::Array(names)
            }
            ("items", s) => canonical_schema(s),
            _ => val.clone(),
        };
        out.insert(k.clone(), val);
    }
    Value::Object(out)
}

/// `v` with every schema `description` removed: what must survive compaction unchanged.
fn without_descriptions(v: &Value) -> Value {
    let Value::Object(map) = v else {
        return v.clone();
    };
    let out: Map<String, Value> = map
        .iter()
        .filter(|(k, _)| *k != "description")
        .map(|(k, val)| {
            let val = match (k.as_str(), val) {
                ("properties", Value::Object(props)) => Value::Object(
                    props
                        .iter()
                        .map(|(name, s)| (name.clone(), without_descriptions(s)))
                        .collect(),
                ),
                ("items", s) => without_descriptions(s),
                _ => val.clone(),
            };
            (k.clone(), val)
        })
        .collect();
    Value::Object(out)
}

/// The block must decode to exactly what was shown, and what was shown must differ from the
/// original in descriptions only.
fn self_check(tools: &[ToolDef], shown: &[Shown], compact: &CompactTools) -> Result<(), Error> {
    let decoded = decode_tools(compact).map_err(|e| unsupported("", format!("self-check: {e}")))?;
    let canon = |p: Option<&Value>| p.map(canonical_schema);
    let bare = |p: Option<&Value>| p.map(|v| without_descriptions(&canonical_schema(v)));
    let same = decoded.len() == tools.len()
        && tools.iter().zip(shown).zip(&decoded).all(|((orig, s), d)| {
            let s_params = s.params.as_ref().map(Node::to_json);
            orig.name == d.name
                && s.description == d.description
                && canon(s_params.as_ref()) == canon(d.parameters.as_ref())
                && bare(orig.parameters.as_ref()) == bare(d.parameters.as_ref())
        });
    if same {
        Ok(())
    } else {
        Err(unsupported(
            "",
            "self-check: compact form does not round-trip",
        ))
    }
}

fn malformed(reason: impl Into<String>) -> Error {
    Error::Malformed(reason.into())
}

fn empty_object(closed: bool) -> Object {
    Object {
        properties: BTreeMap::new(),
        required: BTreeSet::new(),
        closed,
    }
}

/// Tool and field names: ASCII letters, digits, `_`, `-`, `.`.
pub(crate) fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn keyword(f: Format) -> &'static str {
    match f {
        Format::DateTime => "datetime",
        Format::Date => "date",
        Format::Email => "email",
        Format::Uri => "uri",
    }
}

// ---------------------------------------------------------------------------------------
// Descriptions

/// Parse `tool` and trim its descriptions to what the model needs.
fn show(tool: &ToolDef) -> Result<Shown, Error> {
    let name = tool.name.as_str();
    if !is_name(name) {
        return Err(unsupported(name, "tool name outside [A-Za-z0-9_.-]"));
    }
    let mut params = normalize_tool(tool)?;
    if let Some(p) = &mut params {
        if p.description.is_some() {
            return Err(unsupported(name, "description on the parameters object"));
        }
        let Kind::Object(o) = &mut p.kind else {
            return Err(unsupported(name, "parameters are not an object"));
        };
        trim_fields(o, &mut vec![name.to_string()], name)?;
    }
    let description = match &tool.description {
        Some(d) => describe(d, &[name.to_string()], &[], false, name)?,
        None => None,
    };
    Ok(Shown {
        name: name.to_string(),
        description,
        params,
    })
}

/// Trim the descriptions of `obj`'s fields; `names` is the tool name plus the field path.
fn trim_fields(obj: &mut Object, names: &mut Vec<String>, path: &str) -> Result<(), Error> {
    for (field, node) in &mut obj.properties {
        let path = format!("{path}.{field}");
        let optional = !obj.required.contains(field);
        names.push(field.clone());
        if let Some(d) = node.description.take() {
            node.description = describe(&d, names, &implied(&node.kind), optional, &path)?;
        }
        if let Some(o) = innermost_object(node) {
            trim_fields(o, names, &path)?;
        }
        names.pop();
    }
    Ok(())
}

/// The description the model reads, or `None` when every word in it is already given by the
/// names on the line, the type, or filler.
fn describe(
    d: &str,
    names: &[String],
    implied: &[&str],
    optional: bool,
    path: &str,
) -> Result<Option<String>, Error> {
    if d.chars().any(char::is_control) {
        return Err(unsupported(
            path,
            "description contains a control character",
        ));
    }
    let mut d = squeeze(d);
    if optional {
        d = squeeze(&drop_optional(&d));
    }
    let optional_word: &[&str] = if optional { &["optional"] } else { &[] };
    let said: BTreeSet<String> = names
        .iter()
        .map(String::as_str)
        .chain(implied.iter().copied())
        .chain(FILLER.iter().copied())
        .chain(optional_word.iter().copied())
        .flat_map(words)
        .collect();
    Ok(words(&d).iter().any(|w| !said.contains(w)).then_some(d))
}

/// Words a description is said in terms of, for a type.
fn implied(kind: &Kind) -> Vec<&'static str> {
    match kind {
        Kind::String(Some(Format::DateTime)) => {
            vec!["date", "time", "datetime", "timestamp", "iso", "8601"]
        }
        Kind::String(Some(Format::Date)) => vec!["date", "iso", "8601"],
        Kind::String(Some(Format::Email)) => vec!["email", "address"],
        Kind::String(Some(Format::Uri)) => vec!["url", "uri", "link"],
        Kind::Integer | Kind::Number => vec!["number", "count", "integer"],
        Kind::Boolean => vec!["whether", "flag", "boolean"],
        Kind::Array(items) => {
            let mut v = implied(&items.kind);
            v.push("list");
            v
        }
        Kind::String(None) | Kind::Object(_) | Kind::Enum { .. } => Vec::new(),
    }
}

/// Lower-cased alphanumeric words, plurals folded (`emails` → `email`, `addresses` →
/// `address`), so names and descriptions compare by meaning-bearing stem.
fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase();
            if let Some(stem) = w.strip_suffix("sses") {
                format!("{stem}ss")
            } else if w.ends_with("ss") || w.chars().count() <= 3 {
                w
            } else {
                w.strip_suffix('s').map(str::to_string).unwrap_or(w)
            }
        })
        .collect()
}

/// Drop the "optional" marker a description repeats from the `?`.
fn drop_optional(d: &str) -> String {
    let d = d.replace("(optional)", "").replace("(Optional)", "");
    for p in [
        "Optional:",
        "optional:",
        "Optional,",
        "optional,",
        "Optional ",
        "optional ",
    ] {
        if let Some(rest) = d.strip_prefix(p) {
            return rest.to_string();
        }
    }
    d
}

/// Runs of spaces collapsed, leading/trailing spaces and trailing periods dropped. Only
/// spaces are touched; other whitespace is a control character and is rejected first.
fn squeeze(d: &str) -> String {
    let words: Vec<&str> = d.split(' ').filter(|w| !w.is_empty()).collect();
    words.join(" ").trim_end_matches(['.', ' ']).to_string()
}

// ---------------------------------------------------------------------------------------
// Rendering

fn render_tool(s: &Shown, out: &mut String) -> Result<(), Error> {
    out.push_str(&s.name);
    let fields = match &s.params {
        None => {
            out.push_str("()");
            None
        }
        Some(Node {
            kind: Kind::Object(o),
            ..
        }) => {
            if o.closed {
                out.push('!');
            }
            Some(o)
        }
        Some(_) => return Err(unsupported(&s.name, "parameters are not an object")),
    };
    push_description(s.description.as_deref(), out);
    out.push('\n');
    if let Some(o) = fields {
        render_fields(o, 1, &s.name, out)?;
    }
    Ok(())
}

fn render_fields(obj: &Object, depth: usize, path: &str, out: &mut String) -> Result<(), Error> {
    let required = obj
        .properties
        .iter()
        .filter(|(k, _)| obj.required.contains(*k));
    let optional = obj
        .properties
        .iter()
        .filter(|(k, _)| !obj.required.contains(*k));
    for (name, node) in required.chain(optional) {
        let path = format!("{path}.{name}");
        if !is_name(name) {
            return Err(unsupported(&path, "field name outside [A-Za-z0-9_.-]"));
        }
        out.extend(std::iter::repeat_n(' ', depth));
        out.push_str(name);
        if !obj.required.contains(name) {
            out.push('?');
        }
        out.push(':');
        let inner = render_type(node, &path, out)?;
        if let Some(d) = &node.default {
            out.push('=');
            out.push_str(&literal(d, &path)?);
        }
        push_description(node.description.as_deref(), out);
        out.push('\n');
        if let Some(o) = inner {
            render_fields(o, depth + 1, &path, out)?;
        }
    }
    Ok(())
}

/// Render a type expression; returns the object whose fields must follow, if any.
fn render_type<'a>(
    node: &'a Node,
    path: &str,
    out: &mut String,
) -> Result<Option<&'a Object>, Error> {
    match &node.kind {
        Kind::String(None) => out.push_str("str"),
        Kind::String(Some(f)) => out.push_str(keyword(*f)),
        Kind::Integer => out.push_str("int"),
        Kind::Number => out.push_str("num"),
        Kind::Boolean => out.push_str("bool"),
        Kind::Array(items) => {
            if items.description.is_some() {
                return Err(unsupported(path, "description on array items"));
            }
            if items.default.is_some() {
                return Err(unsupported(path, "default on array items"));
            }
            out.push('[');
            let inner = render_type(items, path, out)?;
            out.push(']');
            return Ok(inner);
        }
        Kind::Object(o) => {
            out.push_str(if o.closed { "{}!" } else { "{}" });
            return Ok(Some(o));
        }
        Kind::Enum { values, typed } => render_enum(values, *typed, path, out)?,
    }
    Ok(None)
}

fn render_enum(
    values: &[Value],
    typed: Option<&str>,
    path: &str,
    out: &mut String,
) -> Result<(), Error> {
    if values.len() < 2 {
        return Err(unsupported(path, "single-literal enum"));
    }
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            out.push('|');
        }
        match (typed, v) {
            (Some("string"), Value::String(s)) if is_string_literal(s) => out.push_str(s),
            (Some("string"), Value::String(_)) => out.push_str(&literal(v, path)?),
            (Some("integer"), Value::Number(n)) => out.push_str(&n.to_string()),
            _ => return Err(unsupported(path, "enum without type string or integer")),
        }
    }
    Ok(())
}

/// `v` as a JSON literal on a field line. ` # ` would be read as the description separator.
fn literal(v: &Value, path: &str) -> Result<String, Error> {
    let s = v.to_string();
    if s.contains(" # ") {
        return Err(unsupported(path, format!("literal {s} contains ` # `")));
    }
    Ok(s)
}

fn push_description(d: Option<&str>, out: &mut String) {
    if let Some(d) = d {
        out.push_str(" # ");
        out.push_str(d);
    }
}

/// May appear unquoted in an enum: not whitespace, control or syntax.
fn is_bare(c: char) -> bool {
    !(c.is_whitespace() || c.is_control() || SYNTAX.contains(&c))
}

/// A string enum literal written bare: non-empty, only bare characters, and not something an
/// integer enum would claim. Every other string is written as a JSON string.
fn is_string_literal(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(is_bare)
        && !matches!(serde_json::from_str::<Value>(s), Ok(Value::Number(_)))
}

// ---------------------------------------------------------------------------------------
// Parsing

/// `head # description` → (head, description). Names, types and literals never contain
/// ` # `, so the first one is always the separator; the description is kept verbatim.
fn split_description(line: &str) -> (&str, Option<String>) {
    match line.split_once(" # ") {
        Some((head, d)) => (head, Some(d.to_string())),
        None => (line, None),
    }
}

/// Read the field lines at `depth` into `obj`, stopping at the first shallower line.
fn parse_fields(
    lines: &[&str],
    i: &mut usize,
    depth: usize,
    obj: &mut Object,
) -> Result<(), Error> {
    while let Some(line) = lines.get(*i) {
        let content = line.trim_start_matches(' ');
        let indent = line.len() - content.len();
        if indent < depth {
            return Ok(());
        }
        if indent > depth {
            return Err(malformed(format!("unexpected indentation at `{line}`")));
        }
        *i += 1;
        let (head, description) = split_description(content);
        let (name, ty) = head
            .split_once(':')
            .ok_or_else(|| malformed(format!("field without type `{line}`")))?;
        let (name, optional) = match name.strip_suffix('?') {
            Some(name) => (name, true),
            None => (name, false),
        };
        if !is_name(name) {
            return Err(malformed(format!("bad field name `{line}`")));
        }
        let mut node = parse_field_type(ty)?;
        node.description = description;
        if let Some(o) = innermost_object(&mut node) {
            parse_fields(lines, i, depth + 1, o)?;
        }
        if obj.properties.insert(name.to_string(), node).is_some() {
            return Err(malformed(format!("duplicate field `{name}`")));
        }
        if !optional {
            obj.required.insert(name.to_string());
        }
    }
    Ok(())
}

/// `TYPE` or `TYPE=DEFAULT`.
fn parse_field_type(s: &str) -> Result<Node, Error> {
    let (mut node, rest) = parse_type(s)?;
    if let Some(lit) = rest.strip_prefix('=') {
        let v = serde_json::from_str(lit)
            .map_err(|e| malformed(format!("bad default `{lit}`: {e}")))?;
        node.default = Some(v);
    } else if !rest.is_empty() {
        return Err(malformed(format!("unexpected `{rest}` after type `{s}`")));
    }
    Ok(node)
}

/// Parse one type expression at the start of `s`; returns it and what follows.
fn parse_type(s: &str) -> Result<(Node, &str), Error> {
    let (kind, rest) = if let Some(r) = s.strip_prefix('[') {
        let (items, r) = parse_type(r)?;
        let r = r
            .strip_prefix(']')
            .ok_or_else(|| malformed(format!("unclosed `[` in `{s}`")))?;
        (Kind::Array(Box::new(items)), r)
    } else if let Some(r) = s.strip_prefix("{}!") {
        (Kind::Object(empty_object(true)), r)
    } else if let Some(r) = s.strip_prefix("{}") {
        (Kind::Object(empty_object(false)), r)
    } else {
        parse_scalar(s)?
    };
    let node = Node {
        kind,
        description: None,
        default: None,
    };
    Ok((node, rest))
}

enum Lit {
    Bare(String),
    Quoted(String),
}

/// A keyword type or an enum (`a|"b c"|d`, `1|2`).
fn parse_scalar(s: &str) -> Result<(Kind, &str), Error> {
    let mut lits = Vec::new();
    let mut rest = s;
    loop {
        let (lit, after) = if rest.starts_with('"') {
            let end = quoted_end(rest)
                .ok_or_else(|| malformed(format!("unterminated literal in `{s}`")))?;
            let text = serde_json::from_str(rest.get(..end).unwrap_or_default())
                .map_err(|e| malformed(format!("bad literal in `{s}`: {e}")))?;
            (Lit::Quoted(text), rest.get(end..).unwrap_or_default())
        } else {
            let end = rest
                .char_indices()
                .find(|&(_, c)| !is_bare(c))
                .map_or(rest.len(), |(i, _)| i);
            if end == 0 {
                return Err(malformed(format!("bad type `{s}`")));
            }
            let bare = rest.get(..end).unwrap_or_default().to_string();
            (Lit::Bare(bare), rest.get(end..).unwrap_or_default())
        };
        lits.push(lit);
        match after.strip_prefix('|') {
            Some(next) => rest = next,
            None => {
                rest = after;
                break;
            }
        }
    }
    let kind = match lits.as_slice() {
        [Lit::Bare(k)] => {
            keyword_kind(k).ok_or_else(|| malformed(format!("unknown type `{k}`")))?
        }
        [Lit::Quoted(_)] => return Err(malformed(format!("single-literal enum `{s}`"))),
        _ => enum_kind(lits, s)?,
    };
    Ok((kind, rest))
}

fn keyword_kind(k: &str) -> Option<Kind> {
    Some(match k {
        "str" => Kind::String(None),
        "datetime" => Kind::String(Some(Format::DateTime)),
        "date" => Kind::String(Some(Format::Date)),
        "email" => Kind::String(Some(Format::Email)),
        "uri" => Kind::String(Some(Format::Uri)),
        "int" => Kind::Integer,
        "num" => Kind::Number,
        "bool" => Kind::Boolean,
        _ => return None,
    })
}

/// End offset (exclusive) of the JSON string starting at `s[0] == '"'`.
fn quoted_end(s: &str) -> Option<usize> {
    let mut esc = false;
    for (i, c) in s.char_indices().skip(1) {
        match (esc, c) {
            (true, _) => esc = false,
            (false, '\\') => esc = true,
            (false, '"') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

/// All bare canonical integers make an integer enum; anything else is a string enum, where a
/// literal must be quoted exactly when it cannot be bare.
fn enum_kind(lits: Vec<Lit>, s: &str) -> Result<Kind, Error> {
    let ints: Option<Vec<Value>> = lits
        .iter()
        .map(|l| match l {
            Lit::Bare(b) => serde_json::from_str::<Value>(b)
                .ok()
                .filter(|v| (v.is_i64() || v.is_u64()) && v.to_string().as_str() == b.as_str()),
            Lit::Quoted(_) => None,
        })
        .collect();
    let (values, typed) = match ints {
        Some(values) => (values, "integer"),
        None => {
            let mut values = Vec::new();
            for l in lits {
                let text = match l {
                    Lit::Bare(b) if is_string_literal(&b) => b,
                    Lit::Quoted(q) if !is_string_literal(&q) => q,
                    _ => return Err(malformed(format!("bad enum `{s}`"))),
                };
                values.push(Value::String(text));
            }
            (values, "string")
        }
    };
    let mut seen = BTreeSet::new();
    if values.iter().any(|v| !seen.insert(v.to_string())) {
        return Err(malformed(format!("duplicate enum literal in `{s}`")));
    }
    Ok(Kind::Enum {
        values,
        typed: Some(typed),
    })
}

fn innermost_object(node: &mut Node) -> Option<&mut Object> {
    match &mut node.kind {
        Kind::Object(o) => Some(o),
        Kind::Array(items) => innermost_object(items),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, description: Option<&str>, parameters: Option<Value>) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: description.map(Into::into),
            parameters,
        }
    }

    fn calendar() -> ToolDef {
        tool(
            "create_event",
            Some("Create a calendar event"),
            Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Event title"},
                    "start": {"type": "string", "format": "date-time"},
                    "attendees": {"type": "array", "items": {"type": "string", "format": "email"}},
                    "options": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {"visibility": {"type": "string", "enum": ["public", "private"]}},
                        "required": ["visibility"]
                    }
                },
                "required": ["title", "start"]
            })),
        )
    }

    /// Schemas that must survive encode → decode_tools exactly, descriptions aside (I3).
    fn supported() -> Vec<ToolDef> {
        vec![
            calendar(),
            tool("get_time", None, None),
            tool(
                "ping",
                Some("no args"),
                Some(json!({"type": "object", "properties": {}, "required": []})),
            ),
            tool(
                "strict",
                None,
                Some(json!({"type": "object", "additionalProperties": false})),
            ),
            tool(
                "search",
                Some("Find # things: a|b, c"),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "q": {"type": "string", "description": "query # with hash, commas, and ünïcode"},
                        "limit": {"type": "integer", "enum": [10, 20, -1], "default": 20},
                        "score": {"type": "number", "default": 0.5},
                        "exact": {"type": "boolean", "description": "", "default": true},
                        "day": {"type": "string", "format": "date"},
                        "url": {"type": "string", "format": "uri"},
                        "mode": {"type": "string", "enum": ["a b", "1", "a|b", "", "x=y", "q\"t", "ok"]},
                        "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}},
                        "rows": {
                            "type": "array",
                            "description": "rows",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "k": {"type": "string", "enum": ["x-1", "y.2", "inf"], "default": "inf"},
                                    "deep": {"type": "object", "properties": {"z": {"type": "string", "default": "a#b?"}}}
                                },
                                "required": ["k"]
                            }
                        }
                    },
                    "required": ["q", "rows"]
                })),
            ),
        ]
    }

    #[test]
    fn renders_a2_exactly() {
        let ct = encode_tools(&[calendar(), tool("get_time", None, None)]).unwrap();
        let want = "create_event # Create a calendar event
 start:datetime
 title:str
 attendees?:[email]
 options?:{}!
  visibility:public|private
get_time()
Call tools as <<call NAME {JSON}>> (no XML/tags)";
        assert_eq!(ct.text, want);
    }

    #[test]
    fn descriptions_keep_only_what_the_line_does_not_say() {
        let t = tool(
            "send_email",
            Some("Send an email from the user's account."),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string", "format": "email"}, "description": "Recipient emails"},
                    "cc": {"type": "array", "items": {"type": "string", "format": "email"}, "description": "CC email addresses"},
                    "subject": {"type": "string", "description": "  Subject  line. "},
                    "sent_at": {"type": "string", "format": "date-time", "description": "Sent time, ISO 8601"},
                    "limit": {"type": "integer", "description": "Optional: maximum count..."},
                    "urgent": {"type": "boolean", "description": "Optional flag"},
                    "notes": {"type": "string", "description": "(optional) Free text"},
                    "body": {"type": "string", "description": "Optional body"}
                },
                "required": ["to", "subject", "body"]
            })),
        );
        let ct = encode_tools(std::slice::from_ref(&t)).unwrap();
        let want = "send_email # Send an email from the user's account
 body:str # Optional body
 subject:str # Subject line
 to:[email] # Recipient emails
 cc?:[email]
 limit?:int # maximum count
 notes?:str # Free text
 sent_at?:datetime
 urgent?:bool
Call tools as <<call NAME {JSON}>> (no XML/tags)";
        assert_eq!(ct.text, want);
        let back = decode_tools(&ct).unwrap();
        let bare = |v: &Option<Value>| {
            v.as_ref()
                .map(|v| without_descriptions(&canonical_schema(v)))
        };
        assert_eq!(bare(&back[0].parameters), bare(&t.parameters));
    }

    #[test]
    fn defaults_and_quoted_literals_round_trip() {
        let t = tool(
            "search_food",
            None,
            Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "data_type": {"type": "string", "enum": ["Foundation", "SR Legacy", "a|b", "1", "", "say \"hi\""], "default": "SR Legacy"},
                    "limit": {"type": "integer", "default": 10},
                    "ratio": {"type": "number", "default": 0.5},
                    "exact": {"type": "boolean", "default": false},
                    "port": {"type": "integer", "enum": [443, 8443], "default": 443},
                    "lang": {"type": "string", "default": "en=US"}
                },
                "required": ["query"]
            })),
        );
        let ct = encode_tools(std::slice::from_ref(&t)).unwrap();
        let want = r#"search_food
 query:str
 data_type?:Foundation|"SR Legacy"|"a|b"|"1"|""|"say \"hi\""="SR Legacy"
 exact?:bool=false
 lang?:str="en=US"
 limit?:int=10
 port?:443|8443=443
 ratio?:num=0.5
Call tools as <<call NAME {JSON}>> (no XML/tags)"#;
        assert_eq!(ct.text, want);
        let back = decode_tools(&ct).unwrap();
        assert_eq!(
            back[0].parameters.as_ref().map(canonical_schema),
            t.parameters.as_ref().map(canonical_schema)
        );
    }

    #[test]
    fn i3_round_trip_is_canonically_equal() {
        let tools = supported();
        let decoded = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
        assert_eq!(decoded.len(), tools.len());
        let bare = |v: &Option<Value>| {
            v.as_ref()
                .map(|v| without_descriptions(&canonical_schema(v)))
        };
        for (a, b) in tools.iter().zip(&decoded) {
            assert_eq!(a.name, b.name);
            assert!(
                b.description.is_none()
                    || b.description.as_deref().map(str::to_string)
                        == a.description.as_deref().map(squeeze),
                "{}",
                a.name
            );
            assert_eq!(bare(&a.parameters), bare(&b.parameters), "{}", a.name);
        }
    }

    #[test]
    fn i2_unrenderable_schemas_are_rejected_not_approximated() {
        let field = |schema: Value| {
            tool(
                "t",
                None,
                Some(json!({"type": "object", "properties": {"f": schema}})),
            )
        };
        let cases = vec![
            field(json!({"type": "string", "enum": ["only"]})),
            field(json!({"enum": ["a", "b"]})),
            field(json!({"type": "number", "enum": [1.5, 2.5]})),
            field(json!({"type": "string", "enum": ["a # b", "c"]})),
            field(json!({"type": "string", "default": "x # y"})),
            field(json!({"type": "integer", "default": "5"})),
            field(json!({"type": "string", "default": null})),
            field(json!({"type": "string", "format": "date", "default": "tomorrow"})),
            field(json!({"type": "array", "items": {"type": "string"}, "default": []})),
            field(json!({"type": "array", "items": {"type": "integer", "default": 1}})),
            field(json!({"type": "string", "description": "two\nlines"})),
            field(json!({"type": "array", "items": {"type": "string", "description": "x"}})),
            field(json!({"type": "string", "pattern": "^a$"})),
            tool(
                "t",
                None,
                Some(json!({"type": "object", "properties": {"a b": {"type": "string"}}})),
            ),
            tool(
                "t",
                None,
                Some(json!({"type": "object", "description": "top"})),
            ),
            tool("bad name", None, None),
            tool("t", Some("line\r"), None),
        ];
        for t in cases {
            let err = encode_tools(std::slice::from_ref(&t)).unwrap_err();
            assert_eq!(err.as_label(), "unsupported", "{t:?}");
        }
        let dup = [tool("t", None, None), tool("t", None, None)];
        assert!(encode_tools(&dup).is_err());
        assert!(encode_tools(&[]).is_err());
    }

    #[test]
    fn i7_same_tools_same_bytes() {
        let a = encode_tools(&supported()).unwrap();
        let b = encode_tools(&supported()).unwrap();
        assert_eq!(a, b);
        // Input key order and `required` order do not matter.
        let x: Value = serde_json::from_str(
            r#"{"type":"object","properties":{"b":{"type":"string"},"a":{"type":"integer"}},"required":["b","a"]}"#,
        )
        .unwrap();
        let y: Value = serde_json::from_str(
            r#"{"required":["a","b"],"properties":{"a":{"type":"integer"},"b":{"type":"string"}},"type":"object"}"#,
        )
        .unwrap();
        assert_eq!(
            encode_tools(&[tool("t", None, Some(x))]).unwrap(),
            encode_tools(&[tool("t", None, Some(y))]).unwrap()
        );
    }

    #[test]
    fn i1_encoder_never_panics_on_junk() {
        let mut deep = json!({"type": "string"});
        for _ in 0..100 {
            deep = json!({"type": "array", "items": deep});
        }
        let junk = [
            json!(null),
            json!(1),
            json!("x"),
            json!([]),
            json!({}),
            json!({"type": "object", "properties": null}),
            json!({"type": "object", "properties": {"a": null}}),
            json!({"type": "object", "properties": {"a": {"type": "array", "items": []}}}),
            json!({"type": "object", "required": "a"}),
            json!({"type": "object", "properties": {"a": {"enum": [null, {}]}}}),
            json!({"type": "object", "properties": {"a": {"type": "string", "default": {"x": [1]}}}}),
            json!({"type": "object", "properties": {"a": {"type": "string", "enum": ["\"", "\\", "\u{0}"]}}}),
            json!({"type": "object", "properties": {"a": deep}}),
        ];
        for j in junk {
            let _ = encode_tools(&[tool("t", None, Some(j))]);
        }
    }

    /// Single-character mutations of a real block never panic the parser, and whatever still
    /// parses survives a re-encode with the same names and structure.
    #[test]
    fn fuzz_block_mutations_never_panic() {
        let text = encode_tools(&supported()).unwrap().text;
        let chars: Vec<char> = text.chars().collect();
        for i in 0..=chars.len() {
            let mut variants = Vec::new();
            if i < chars.len() {
                let mut del = chars.clone();
                del.remove(i);
                variants.push(del);
            }
            for c in ['"', '\\', '|', '=', '[', ']', ' ', '\n', '#', 'é'] {
                let mut ins = chars.clone();
                ins.insert(i, c);
                variants.push(ins);
            }
            for v in variants {
                let block = CompactTools {
                    text: v.into_iter().collect(),
                };
                let bare = |t: &ToolDef| {
                    let p = t.parameters.as_ref();
                    (
                        t.name.clone(),
                        p.map(|v| without_descriptions(&canonical_schema(v))),
                    )
                };
                if let Ok(tools) = decode_tools(&block)
                    && let Ok(again) = encode_tools(&tools)
                {
                    let back = decode_tools(&again).unwrap();
                    let want: Vec<_> = tools.iter().map(bare).collect();
                    let got: Vec<_> = back.iter().map(bare).collect();
                    assert_eq!(got, want, "{:?}", block.text);
                }
            }
        }
    }

    #[test]
    fn decode_tools_rejects_malformed_blocks() {
        let wrap = |body: &str| CompactTools {
            text: format!("{body}\n{INSTRUCTION}"),
        };
        for body in [
            "t\n  a:str",
            "t\n a",
            "t\n a:string",
            "t\n a:x",
            "t\n a:str\n a:int",
            "t\n a:1|1",
            "t\n a:\"x\"|b",
            "t\n a:\"x",
            "t\n a:\"x\"",
            "t\n a:[str",
            "t\n a:str extra",
            "t\n a:int=",
            "t\n a:int=x",
            "t()\n a:str",
            "t\n a:str\n  b:int",
            "",
            " t",
        ] {
            assert!(decode_tools(&wrap(body)).is_err(), "{body:?}");
        }
        assert!(decode_tools(&CompactTools { text: "t".into() }).is_err());
    }
}
