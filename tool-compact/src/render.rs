//! Encoder: [`Ty`] → one compact signature line per tool, plus the call instructions.

use serde_json::{Number, Value};

use crate::ty::{Bounds, Field, Obj, Ty, is_valid_name};
use crate::types::ToolCall;

/// Told to the model once per request. Kept short: it is paid on every request.
pub(crate) const INSTRUCTIONS: &str =
    "To call a tool, reply with <<call NAME {JSON args}>>, one per call. ? = optional.";

/// Type keywords. A bare enum value may not collide with one of these.
pub(crate) const KEYWORDS: &[&str] = &[
    "any", "string", "int", "number", "bool", "null", "object", "datetime", "date", "email", "uri",
];

/// Words that never carry meaning on their own in a field description.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "of", "for", "to", "in", "on", "at", "by", "with", "and", "or", "is", "this",
    "that", "its",
];

/// `name(params) - description`
pub(crate) fn render_tool(name: &str, desc: Option<&str>, params: &Obj) -> String {
    let mut out = format!("{name}({})", render_params(params, name));
    if let Some(d) = desc {
        out.push_str(" - ");
        out.push_str(d);
    }
    out
}

fn render_params(obj: &Obj, tool: &str) -> String {
    let mut parts: Vec<String> = obj.fields.iter().map(|f| render_field(f, tool)).collect();
    if obj.open {
        parts.push("...".into());
    }
    parts.join(", ")
}

fn render_field(f: &Field, tool: &str) -> String {
    let mut out = format!(
        "{}{}:{}",
        f.name,
        if f.required { "" } else { "?" },
        render_type(&f.ty, tool)
    );
    if let Some(d) = &f.default {
        out.push('=');
        out.push_str(&render_default(d));
    }
    if let Some(d) = f
        .desc
        .as_deref()
        .filter(|d| !is_redundant(d, &f.name, tool))
    {
        out.push(' ');
        out.push_str(&render_desc(d));
    }
    out
}

/// `(text)` when its parentheses balance, otherwise a JSON string.
fn render_desc(d: &str) -> String {
    let mut depth = 0i32;
    let balanced = d.chars().all(|c| {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        depth >= 0
    }) && depth == 0;
    if balanced {
        format!("({d})")
    } else {
        Value::String(d.to_string()).to_string()
    }
}

pub(crate) fn render_type(ty: &Ty, tool: &str) -> String {
    match ty {
        Ty::Any => "any".into(),
        Ty::Str(None) => "string".into(),
        Ty::Str(Some(f)) => f.keyword().into(),
        Ty::Int => "int".into(),
        Ty::Num => "number".into(),
        Ty::Bool => "bool".into(),
        Ty::Null => "null".into(),
        Ty::Enum(values) => values
            .iter()
            .map(render_literal)
            .collect::<Vec<_>>()
            .join("|"),
        Ty::Arr(inner) => format!("{}[]", wrap(inner, tool)),
        Ty::Obj(o) if o.fields.is_empty() => if o.open { "object" } else { "{}" }.into(),
        Ty::Obj(o) => format!("{{{}}}", render_params(o, tool)),
        Ty::Nullable(inner) => format!("{}|null", wrap(inner, tool)),
        Ty::Bounded(inner, b) => format!("{}{}", render_type(inner, tool), render_bounds(b)),
    }
}

/// `(lo..hi)` directly after the type, either side optional; `>` / `<` mark an exclusive limit.
/// On a string or array the limits are its length.
fn render_bounds(b: &Bounds) -> String {
    let side = |n: &Option<Number>, exclusive: bool, mark: &str| match n {
        Some(n) if exclusive => format!("{mark}{n}"),
        Some(n) => n.to_string(),
        None => String::new(),
    };
    format!(
        "({}..{})",
        side(&b.min, b.min_exclusive, ">"),
        side(&b.max, b.max_exclusive, "<")
    )
}

/// A default renders bare when unambiguous (`celsius`), as JSON otherwise (`10`, `true`, `"a b"`).
fn render_default(v: &Value) -> String {
    match v.as_str() {
        Some(s) if is_bare_literal(s) && !matches!(s, "true" | "false") => s.to_string(),
        _ => v.to_string(),
    }
}

fn wrap(ty: &Ty, tool: &str) -> String {
    let s = render_type(ty, tool);
    match ty {
        Ty::Enum(v) if v.len() > 1 => format!("({s})"),
        Ty::Nullable(_) => format!("({s})"),
        _ => s,
    }
}

/// Enum values render bare when unambiguous (`public`), as JSON otherwise (`"in progress"`).
fn render_literal(v: &Value) -> String {
    match v.as_str() {
        Some(s) if is_bare_literal(s) => s.to_string(),
        _ => v.to_string(),
    }
}

pub(crate) fn is_bare_literal(s: &str) -> bool {
    is_valid_name(s) && !s.starts_with(|c: char| c.is_ascii_digit()) && !KEYWORDS.contains(&s)
}

/// A field description that only repeats the field and tool names adds no meaning.
/// ("Event title" on `title` in `create_calendar_event`.) Anything else is kept.
fn is_redundant(desc: &str, field: &str, tool: &str) -> bool {
    let known: Vec<String> = words(field).chain(words(tool)).collect();
    let mut desc_words = words(desc)
        .filter(|w| !STOP_WORDS.contains(&w.as_str()))
        .peekable();
    desc_words.peek().is_some() && desc_words.all(|w| known.contains(&w))
}

/// Lower-cased words, split on non-alphanumerics and camelCase, with a trailing plural `s` dropped.
fn words(s: &str) -> impl Iterator<Item = String> {
    let mut spaced = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            spaced.push(' ');
        }
        prev_lower = c.is_ascii_lowercase();
        spaced.push(if c.is_alphanumeric() { c } else { ' ' });
    }
    spaced
        .split_whitespace()
        .map(|w| {
            let w = w.to_lowercase();
            match w.strip_suffix('s') {
                Some(stem) if stem.len() > 2 && !stem.ends_with('s') => stem.to_string(),
                _ => w,
            }
        })
        .collect::<Vec<_>>()
        .into_iter()
}

/// Calls written in the compact call grammar, one per line.
pub fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(|c| format!("<<call {} {}>>", c.name, c.arguments_json()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::from_schema;
    use serde_json::json;

    fn render(schema: Value) -> String {
        let Ty::Obj(o) = from_schema(&schema, "p").unwrap() else {
            panic!()
        };
        render_tool("create_calendar_event", Some("Create an event."), &o)
    }

    #[test]
    fn renders_a_signature_with_required_fields_first() {
        let out = render(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        }));
        assert_eq!(
            out,
            "create_calendar_event(start:datetime (Start time, ISO 8601), title:string, \
             attendees?:string[], visibility?:public|private) - Create an event."
        );
    }

    #[test]
    fn renders_nested_and_unusual_types() {
        let out = render(json!({
            "type": "object",
            "properties": {
                "items": {"type": "array", "description": "Line items (one per SKU)", "items": {
                    "type": "object",
                    "properties": {"sku": {"type": "string"}, "qty": {"type": "integer"}},
                    "required": ["sku"]
                }},
                "meta": {"type": "object"},
                "note": {"type": ["string", "null"], "description": "Shown :) to users"},
                "tags": {"type": "array", "items": {"enum": ["a", "in progress", "null", 3]}}
            }
        }));
        assert!(
            out.contains("items?:{sku:string, qty?:int}[] (Line items (one per SKU))"),
            "{out}"
        );
        assert!(out.contains("meta?:object"), "{out}");
        assert!(
            out.contains("note?:string|null \"Shown :) to users\""),
            "{out}"
        );
        assert!(
            out.contains("tags?:(a|\"in progress\"|\"null\"|3)[]"),
            "{out}"
        );
    }

    #[test]
    fn drops_only_redundant_descriptions() {
        assert!(is_redundant(
            "Event title",
            "title",
            "create_calendar_event"
        ));
        assert!(is_redundant("The titles", "title", "x"));
        assert!(!is_redundant("Recipient emails", "to", "send_email"));
        assert!(!is_redundant(
            "Duration in minutes",
            "duration_min",
            "create_calendar_event"
        ));
        assert!(!is_redundant("", "title", "x"));
    }

    #[test]
    fn bare_literals_never_collide() {
        assert!(is_bare_literal("public") && is_bare_literal("dry-run"));
        assert!(!is_bare_literal("null") && !is_bare_literal("string") && !is_bare_literal("2fa"));
        assert!(!is_bare_literal("in progress") && !is_bare_literal(""));
    }
}
