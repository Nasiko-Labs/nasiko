//! Rendering: schema tree → compact definitions text, and calls → call text.
//!
//! The grammar is specified in the crate docs (`lib.rs`); [`crate::defs`] is its inverse.

use serde_json::Value;

use crate::schema::{Kind, Node, Obj};

/// Call-format instructions placed before the definitions.
///
/// This text is paid on every request, so it is cut to the clauses a model cannot infer: the
/// call shape, that arguments are JSON, that several calls are allowed, that the reply stops
/// after them (otherwise models narrate imagined results), the no-call escape hatch, and `?`.
/// The rest of the notation (`T[]`, `'a'|'b'`, indented fields) reads as TypeScript/YAML and
/// is left implicit. A 100-token version with a full notation key cost 8 points of token
/// reduction on the public eval set.
pub const INSTRUCTIONS: &str = "\
To use a tool, reply <<call NAME {JSON args}>>, always with the word call and closing >>. \
Several allowed; stop after the last. Otherwise answer normally.";

/// Opening marker of a call.
pub const CALL_OPEN: &str = "<<call";
/// Closing marker of a call.
pub const CALL_CLOSE: &str = ">>";

/// Render a scalar literal (enum member or default) in the compact grammar.
///
/// Strings are single-quoted so they cost no `\"` escapes once the definitions are embedded
/// in a JSON request body.
pub(crate) fn render_literal(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
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
        }
        // Only scalars reach here (enforced by `Node::from_json`).
        other => out.push_str(&other.to_string()),
    }
}

fn render_ann(node: &Node, out: &mut String) {
    let mut items: Vec<String> = Vec::new();
    if let Kind::Object(Obj { closed: true, .. }) = node.kind {
        items.push("closed".into());
    }
    let a = &node.ann;
    if let Some(f) = &a.format {
        items.push(f.clone());
    }
    if let Some(n) = &a.minimum {
        items.push(format!("min={n}"));
    }
    if let Some(n) = &a.maximum {
        items.push(format!("max={n}"));
    }
    for (key, v) in [
        ("minLen", a.min_length),
        ("maxLen", a.max_length),
        ("minItems", a.min_items),
        ("maxItems", a.max_items),
    ] {
        if let Some(v) = v {
            items.push(format!("{key}={v}"));
        }
    }
    if let Some(p) = &a.pattern {
        let mut s = String::from("pattern=");
        render_literal(&Value::String(p.clone()), &mut s);
        items.push(s);
    }
    if let Some(d) = &a.default {
        let mut s = String::from("default=");
        render_literal(d, &mut s);
        items.push(s);
    }
    if !items.is_empty() {
        out.push('(');
        out.push_str(&items.join(","));
        out.push(')');
    }
}

/// Whether a node renders as a `|` union and needs parentheses as an array item.
fn is_union(node: &Node) -> bool {
    match &node.kind {
        Kind::Types(b) => b.len() > 1,
        Kind::Enum(v) => v.len() > 1,
        _ => false,
    }
}

/// Render a type expression (no description, no children).
pub(crate) fn render_type(node: &Node, out: &mut String) {
    match &node.kind {
        Kind::Any => out.push_str("any"),
        Kind::Types(bases) => {
            let names: Vec<&str> = bases.iter().map(|b| b.keyword()).collect();
            out.push_str(&names.join("|"));
        }
        Kind::Enum(values) => {
            for (i, v) in values.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                render_literal(v, out);
            }
        }
        Kind::Object(_) => out.push_str("obj"),
        Kind::Array(item) => {
            match item {
                None => out.push_str("any"),
                Some(item) if is_union(item) => {
                    out.push('(');
                    render_type(item, out);
                    out.push(')');
                }
                Some(item) => render_type(item, out),
            }
            out.push_str("[]");
        }
    }
    render_ann(node, out);
}

/// The object whose fields are listed on the indented lines below a field, if any: the field
/// itself, or the innermost item of an array-of-objects.
pub(crate) fn child_object(node: &Node) -> Option<&Obj> {
    match &node.kind {
        Kind::Object(obj) => Some(obj),
        Kind::Array(Some(item)) => child_object(item),
        _ => None,
    }
}

/// Lowercase words of an identifier or sentence: split on non-alphanumerics and camelCase.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Plural-insensitive comparison form (`cities`/`city`, `boxes`/`box`, `emails`/`email`).
fn stem(w: &str) -> String {
    if let Some(s) = w.strip_suffix("ies").filter(|s| s.len() > 1) {
        return format!("{s}y");
    }
    for suffix in ["ses", "xes", "zes", "ches", "shes"] {
        if w.len() > suffix.len() + 1 && w.ends_with(suffix) {
            return w[..w.len() - 2].to_string();
        }
    }
    match w.strip_suffix('s') {
        Some(s) if s.len() > 2 && !s.ends_with('s') => s.to_string(),
        _ => w.to_string(),
    }
}

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "for", "in", "on", "to", "by", "with", "and", "or", "is", "this",
];

/// Whether a field description only restates its own name and its tool's name (`title` in
/// `create_calendar_event`: "Event title"). Such a description cannot disambiguate anything,
/// so the brief allows dropping it; every description with one new content word is kept.
pub fn is_redundant_description(desc: &str, field: &str, tool: &str) -> bool {
    let known: Vec<String> = words(field)
        .into_iter()
        .chain(words(tool))
        .map(|w| stem(&w))
        .collect();
    let content: Vec<String> = words(desc)
        .into_iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect();
    !content.is_empty() && content.iter().all(|w| known.contains(&stem(w)))
}

fn render_fields(obj: &Obj, tool: &str, depth: usize, out: &mut String) {
    for p in &obj.props {
        out.push('\n');
        for _ in 0..depth {
            out.push(' ');
        }
        out.push_str(&p.name);
        if !p.required {
            out.push('?');
        }
        out.push_str(": ");
        render_type(&p.node, out);
        if child_object(&p.node).is_some() {
            out.push_str(" {");
        }
        if let Some(d) = &p.node.description
            && !is_redundant_description(d, &p.name, tool)
        {
            out.push_str(" # ");
            out.push_str(d);
        }
        if let Some(child) = child_object(&p.node) {
            render_fields(child, tool, depth + 1, out);
            // Indentation alone did not survive live testing: three models from different
            // providers put a field that follows a nested object *inside* it. Braces mark
            // where the object ends, as in a TypeScript interface.
            out.push('\n');
            for _ in 0..depth {
                out.push(' ');
            }
            out.push('}');
        }
    }
}

/// Render one tool: a header line, then one line per field.
pub(crate) fn render_tool(name: &str, description: Option<&str>, params: &Obj, out: &mut String) {
    out.push_str(name);
    if params.closed {
        out.push_str("(closed)");
    }
    out.push(':');
    if let Some(d) = description {
        out.push(' ');
        out.push_str(d);
    }
    render_fields(params, name, 1, out);
}

/// Render one call in the call grammar. `arguments` must be a JSON object; it is written as
/// compact JSON.
pub fn render_call(name: &str, arguments: &Value) -> String {
    format!("{CALL_OPEN} {name} {arguments}{CALL_CLOSE}")
}

/// Render several calls, one per line — the text a model is expected to produce for them.
///
/// Used for the eval's `rendered_calls` and by the router to replay previous assistant calls
/// in conversation history.
pub fn render_calls(calls: &[crate::ToolCall]) -> crate::Result<String> {
    let mut lines = Vec::with_capacity(calls.len());
    for call in calls {
        let args: Value = if call.arguments.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&call.arguments).map_err(|e| crate::Error::InvalidArguments {
                tool: call.name.clone(),
                reason: format!("arguments are not JSON: {e}"),
            })?
        };
        if !args.is_object() {
            return Err(crate::Error::InvalidArguments {
                tool: call.name.clone(),
                reason: "arguments are not a JSON object".into(),
            });
        }
        lines.push(render_call(&call.name, &args));
    }
    Ok(lines.join("\n"))
}
