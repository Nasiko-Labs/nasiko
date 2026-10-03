//! AST → one canonical line of compact notation.
//!
//! The output is canonical: for a given AST there is exactly one rendering, and
//! [`crate::parse`] accepts exactly that rendering. Object properties are written required-first
//! in `required` order, then the rest sorted by name, so the `required` array round-trips with no
//! extra marker. All iteration over JSON maps goes through explicitly sorted key lists, never the
//! map's own order.

use serde_json::Value;

use crate::json::canonical_json;
use crate::lexeme::{alias_for_format, is_bare_word, is_raw_safe, quote, quote_description};
use crate::schema::{Annot, Kind, Node, ObjectSchema, Scalar, ScalarBase};
use crate::types::ToolDef;

/// Render a tool whose `parameters` (if any) have been lowered to `schema`.
pub(crate) fn render_tool(def: &ToolDef, schema: Option<&Node>) -> String {
    let mut out = String::new();
    out.push_str(&def.name);
    if let Some(node) = schema {
        render_params(&mut out, node);
        render_annot(&mut out, &node.annot);
        if let Some(d) = &node.description {
            out.push(' ');
            out.push_str(&quote_description(d));
        }
    }
    if let Some(d) = &def.description {
        out.push_str(" - ");
        if is_raw_safe(d) {
            out.push_str(d);
        } else {
            out.push_str(&quote(d));
        }
    }
    out
}

fn render_params(out: &mut String, node: &Node) {
    match &node.kind {
        Kind::Any => out.push_str("(*)"),
        Kind::Object(o) => {
            out.push('(');
            render_object_body(out, o);
            out.push(')');
            render_flags(out, o);
        }
        // `Node::from_value` only returns Any or Object at the root.
        _ => out.push_str("(*)"),
    }
}

fn render_object_body(out: &mut String, o: &ObjectSchema) {
    match &o.properties {
        None => out.push_str("..."),
        Some(props) => {
            let mut first = true;
            let mut push = |out: &mut String, name: &str, node: &Node, required: bool| {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(name);
                if !required {
                    out.push('?');
                }
                out.push(':');
                render_typed(out, node);
            };
            if let Some(required) = &o.required {
                for name in required {
                    if let Some((n, node)) = props.iter().find(|(n, _)| n == name) {
                        push(out, n, node, true);
                    }
                }
            }
            for (name, node) in props {
                if !o.is_required(name) {
                    push(out, name, node, false);
                }
            }
        }
    }
}

fn render_flags(out: &mut String, o: &ObjectSchema) {
    match o.additional {
        Some(false) => out.push('!'),
        Some(true) => out.push('+'),
        None => {}
    }
    if o.required.as_ref().is_some_and(Vec::is_empty) {
        out.push('=');
    }
}

fn render_annot(out: &mut String, annot: &Annot) {
    if annot.is_empty() {
        return;
    }
    out.push_str("@{");
    let mut first = true;
    let mut field = |out: &mut String, key: &str, v: &Value| {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&quote(key));
        out.push(':');
        out.push_str(&canonical_json(v));
    };
    if let Some(t) = &annot.title {
        field(out, "title", &Value::String(t.clone()));
    }
    if let Some(d) = &annot.default {
        field(out, "default", d);
    }
    if let Some(e) = &annot.examples {
        field(out, "examples", e);
    }
    out.push('}');
}

/// A type expression followed by its annotations and description, as used for properties and
/// array items.
fn render_typed(out: &mut String, node: &Node) {
    render_type(out, node);
    render_annot(out, &node.annot);
    if let Some(d) = &node.description {
        out.push(' ');
        out.push_str(&quote_description(d));
    }
}

fn render_type(out: &mut String, node: &Node) {
    match &node.kind {
        Kind::Any => out.push('*'),
        Kind::Scalar(s) => {
            render_scalar(out, s);
            if node.nullable {
                out.push_str("|null");
            }
        }
        Kind::Enum { members, .. } => {
            let mut first = true;
            for m in members {
                if !first {
                    out.push('|');
                }
                first = false;
                match m {
                    Value::String(s) if is_bare_word(s) => out.push_str(s),
                    Value::String(s) => out.push_str(&quote(s)),
                    Value::Number(n) => out.push_str(&n.to_string()),
                    Value::Null => out.push_str("null"),
                    // Lowering rejects anything else.
                    other => out.push_str(&canonical_json(other)),
                }
            }
        }
        Kind::Array {
            items,
            min_items,
            max_items,
        } => {
            out.push('[');
            match items {
                Some(items) => render_typed(out, items),
                None => out.push_str("..."),
            }
            out.push(']');
            if min_items.is_some() || max_items.is_some() {
                out.push('(');
                if let Some(min) = min_items {
                    out.push_str(&min.to_string());
                }
                out.push_str("..");
                if let Some(max) = max_items {
                    out.push_str(&max.to_string());
                }
                out.push(')');
            }
            if node.nullable {
                out.push_str("|null");
            }
        }
        Kind::Object(o) => {
            out.push('{');
            render_object_body(out, o);
            out.push('}');
            render_flags(out, o);
            if node.nullable {
                out.push_str("|null");
            }
        }
    }
}

fn render_scalar(out: &mut String, s: &Scalar) {
    match s.base {
        ScalarBase::Str => {
            let alias = s.format.as_deref().and_then(alias_for_format);
            out.push_str(alias.unwrap_or("str"));
            let mut parts: Vec<String> = Vec::new();
            if let Some(min) = s.min_length {
                parts.push(format!("min={min}"));
            }
            if let Some(max) = s.max_length {
                parts.push(format!("max={max}"));
            }
            if alias.is_none()
                && let Some(f) = &s.format
            {
                let f = if is_bare_word(f) { f.clone() } else { quote(f) };
                parts.push(format!("format={f}"));
            }
            if !parts.is_empty() {
                out.push('(');
                out.push_str(&parts.join(","));
                out.push(')');
            }
        }
        ScalarBase::Int | ScalarBase::Num => {
            out.push_str(s.base.word());
            if s.has_numeric_bounds() {
                out.push('(');
                if s.has_exclusive_bounds() {
                    let mut parts: Vec<String> = Vec::new();
                    if let Some(n) = &s.minimum {
                        parts.push(format!("ge={n}"));
                    }
                    if let Some(n) = &s.exclusive_minimum {
                        parts.push(format!("gt={n}"));
                    }
                    if let Some(n) = &s.maximum {
                        parts.push(format!("le={n}"));
                    }
                    if let Some(n) = &s.exclusive_maximum {
                        parts.push(format!("lt={n}"));
                    }
                    out.push_str(&parts.join(","));
                } else {
                    if let Some(n) = &s.minimum {
                        out.push_str(&n.to_string());
                    }
                    out.push_str("..");
                    if let Some(n) = &s.maximum {
                        out.push_str(&n.to_string());
                    }
                }
                out.push(')');
            }
        }
        ScalarBase::Bool | ScalarBase::Null => out.push_str(s.base.word()),
    }
}
