//! The A2 compact form: rendering, parsing it back (`decode_tools`) and the encode-time
//! self-check.
//!
//! ```text
//! Tools (? = optional):
//! create_event # Create a calendar event
//!  title:str # Event title
//!  start:datetime
//!  attendees?:[email]
//!  options?:{}!
//!   visibility:public|private
//! get_time()
//! Call tools ONLY as <<call NAME {JSON args}>> (one per call, never XML/tags); else answer normally.
//! ```
//!
//! * Tool line: `NAME` then `()` (no parameters) or `!` (closed top-level object), then an
//!   optional ` # DESCRIPTION`.
//! * Field line: one space of indent per nesting level, `NAME`, `?` if optional, `:TYPE`,
//!   optional ` # DESCRIPTION`. Required fields come first, then optional, each alphabetical.
//! * TYPE: `str int num bool datetime date email uri`, `[TYPE]` (array), `{}` (object; its
//!   fields follow one level deeper), `{}!` (object with `additionalProperties: false`),
//!   `a|b|c` (string enum) or `1|2|3` (integer enum).
//!
//! Anything this cannot state exactly is `Error::Unsupported` and the caller sends native tools.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::schema::{Format, Kind, Node, Object, normalize_tool, unsupported};
use crate::{Error, ToolDef};

/// First line of every compact block.
pub const HEADER: &str = "Tools (? = optional):";
/// Last line of every compact block: how the model calls a tool.
pub const INSTRUCTION: &str = "Call tools ONLY as <<call NAME {JSON args}>> (one per call, never XML/tags); else answer normally.";

/// Characters that are grammar syntax and so cannot appear inside an enum literal.
const SYNTAX: &[char] = &['|', '#', ':', '?', '!', '[', ']', '{', '}', '(', ')'];

/// Tool definitions rendered in the compact form, ready to go in the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTools {
    pub text: String,
}

/// Render `tools` compactly. Fails with `Error::Unsupported` if any tool cannot be stated
/// exactly, or if the result does not decode back to the same schemas (self-check).
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools, Error> {
    if tools.is_empty() {
        return Err(unsupported("", "no tools"));
    }
    let mut seen = BTreeSet::new();
    let mut text = format!("{HEADER}\n");
    for tool in tools {
        if !seen.insert(tool.name.as_str()) {
            return Err(unsupported(&tool.name, "duplicate tool name"));
        }
        render_tool(tool, &mut text)?;
    }
    text.push_str(INSTRUCTION);
    let compact = CompactTools { text };
    self_check(tools, &compact)?;
    Ok(compact)
}

/// Parse a compact block back into tool definitions with canonical JSON Schemas.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, Error> {
    let lines: Vec<&str> = compact.text.split('\n').collect();
    let body = match lines.as_slice() {
        [first, body @ .., last] if *first == HEADER && *last == INSTRUCTION => body,
        _ => return Err(malformed("missing header or instruction line")),
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

fn self_check(tools: &[ToolDef], compact: &CompactTools) -> Result<(), Error> {
    let decoded = decode_tools(compact).map_err(|e| unsupported("", format!("self-check: {e}")))?;
    let same = decoded.len() == tools.len()
        && tools.iter().zip(&decoded).all(|(a, b)| {
            a.name == b.name
                && a.description == b.description
                && a.parameters.as_ref().map(canonical_schema)
                    == b.parameters.as_ref().map(canonical_schema)
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
// Rendering

fn render_tool(tool: &ToolDef, out: &mut String) -> Result<(), Error> {
    let path = tool.name.as_str();
    if !is_name(path) {
        return Err(unsupported(path, "tool name outside [A-Za-z0-9_.-]"));
    }
    out.push_str(path);
    let fields = match normalize_tool(tool)? {
        None => {
            out.push_str("()");
            None
        }
        Some(Node {
            description: Some(_),
            ..
        }) => return Err(unsupported(path, "description on the parameters object")),
        Some(Node {
            kind: Kind::Object(o),
            ..
        }) => {
            if o.closed {
                out.push('!');
            }
            Some(o)
        }
        Some(_) => return Err(unsupported(path, "parameters are not an object")),
    };
    push_description(tool.description.as_deref(), path, out)?;
    out.push('\n');
    if let Some(o) = fields {
        render_fields(&o, 1, path, out)?;
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
        push_description(node.description.as_deref(), &path, out)?;
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
            (Some("string"), _) => {
                return Err(unsupported(
                    path,
                    format!("enum literal {v} clashes with syntax"),
                ));
            }
            (Some("integer"), Value::Number(n)) => out.push_str(&n.to_string()),
            _ => return Err(unsupported(path, "enum without type string or integer")),
        }
    }
    Ok(())
}

fn push_description(d: Option<&str>, path: &str, out: &mut String) -> Result<(), Error> {
    if let Some(d) = d {
        if d.chars().any(char::is_control) {
            return Err(unsupported(
                path,
                "description contains a control character",
            ));
        }
        out.push_str(" # ");
        out.push_str(d);
    }
    Ok(())
}

/// A string enum literal that reads back unambiguously: no whitespace, control or syntax
/// characters, and not something an integer enum would claim.
fn is_string_literal(s: &str) -> bool {
    !s.is_empty()
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || SYNTAX.contains(&c))
        && !matches!(serde_json::from_str::<Value>(s), Ok(Value::Number(_)))
}

// ---------------------------------------------------------------------------------------
// Parsing

/// `head # description` → (head, description). Names and types contain no spaces, so the
/// first ` # ` is always the separator; the description is kept verbatim.
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
        let mut node = parse_type(ty)?;
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

fn parse_type(s: &str) -> Result<Node, Error> {
    let kind = if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        Kind::Array(Box::new(parse_type(inner)?))
    } else {
        match s {
            "str" => Kind::String(None),
            "datetime" => Kind::String(Some(Format::DateTime)),
            "date" => Kind::String(Some(Format::Date)),
            "email" => Kind::String(Some(Format::Email)),
            "uri" => Kind::String(Some(Format::Uri)),
            "int" => Kind::Integer,
            "num" => Kind::Number,
            "bool" => Kind::Boolean,
            "{}" => Kind::Object(empty_object(false)),
            "{}!" => Kind::Object(empty_object(true)),
            _ => parse_enum(s)?,
        }
    };
    Ok(Node {
        kind,
        description: None,
    })
}

fn parse_enum(s: &str) -> Result<Kind, Error> {
    let literals: Vec<&str> = s.split('|').collect();
    if literals.len() < 2 {
        return Err(malformed(format!("unknown type `{s}`")));
    }
    let mut seen = BTreeSet::new();
    if literals.iter().any(|l| !seen.insert(*l)) {
        return Err(malformed(format!("duplicate enum literal in `{s}`")));
    }
    let ints: Vec<Value> = literals
        .iter()
        .filter_map(|l| {
            serde_json::from_str::<Value>(l)
                .ok()
                .filter(|v| (v.is_i64() || v.is_u64()) && v.to_string().as_str() == *l)
        })
        .collect();
    if ints.len() == literals.len() {
        return Ok(Kind::Enum {
            values: ints,
            typed: Some("integer"),
        });
    }
    if !literals.iter().all(|l| is_string_literal(l)) {
        return Err(malformed(format!("bad enum `{s}`")));
    }
    Ok(Kind::Enum {
        values: literals
            .iter()
            .map(|l| Value::String(l.to_string()))
            .collect(),
        typed: Some("string"),
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

    /// Schemas that must survive encode → decode_tools exactly (I3).
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
                        "limit": {"type": "integer", "enum": [10, 20, -1]},
                        "score": {"type": "number"},
                        "exact": {"type": "boolean", "description": ""},
                        "day": {"type": "string", "format": "date"},
                        "url": {"type": "string", "format": "uri"},
                        "matrix": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}},
                        "rows": {
                            "type": "array",
                            "description": "rows",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "k": {"type": "string", "enum": ["x-1", "y.2", "inf"]},
                                    "deep": {"type": "object", "properties": {"z": {"type": "string"}}}
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
        let want = "Tools (? = optional):
create_event # Create a calendar event
 start:datetime
 title:str # Event title
 attendees?:[email]
 options?:{}!
  visibility:public|private
get_time()
Call tools ONLY as <<call NAME {JSON args}>> (one per call, never XML/tags); else answer normally.";
        assert_eq!(ct.text, want);
    }

    #[test]
    fn i3_round_trip_is_canonically_equal() {
        let tools = supported();
        let decoded = decode_tools(&encode_tools(&tools).unwrap()).unwrap();
        assert_eq!(decoded.len(), tools.len());
        for (a, b) in tools.iter().zip(&decoded) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.description, b.description);
            assert_eq!(
                a.parameters.as_ref().map(canonical_schema),
                b.parameters.as_ref().map(canonical_schema),
                "{}",
                a.name
            );
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
            field(json!({"type": "string", "enum": ["a b", "c"]})),
            field(json!({"type": "string", "enum": ["1", "a"]})),
            field(json!({"type": "string", "enum": ["a|b", "c"]})),
            field(json!({"type": "string", "enum": ["", "c"]})),
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
            json!({"type": "object", "properties": {"a": deep}}),
        ];
        for j in junk {
            let _ = encode_tools(&[tool("t", None, Some(j))]);
        }
    }

    #[test]
    fn decode_tools_rejects_malformed_blocks() {
        let wrap = |body: &str| CompactTools {
            text: format!("{HEADER}\n{body}\n{INSTRUCTION}"),
        };
        for body in [
            "t\n  a:str",
            "t\n a",
            "t\n a:string",
            "t\n a:x",
            "t\n a:str\n a:int",
            "t\n a:1|1",
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
