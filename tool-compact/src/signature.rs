//! The one-line signature grammar (see `README.md`): render a tool from [`Node`]s and parse it
//! back. Free text (descriptions, patterns) is JSON-quoted wherever it could collide with the
//! grammar, so parsing never has to guess where it ends.

use serde_json::{Number, Value};

use crate::cursor::Cursor;
use crate::schema::{Cons, Field, MAX_DEPTH, Node, Obj, Ty, is_enum_char, is_name_char};

/// A parsed tool line.
pub(crate) struct Signature {
    pub(crate) name: String,
    pub(crate) desc: Option<String>,
    pub(crate) params: Obj,
}

/// Render `name(fields)[!] - desc`.
pub(crate) fn render(name: &str, desc: Option<&str>, params: &Obj, paren_desc: bool) -> String {
    let mut out = String::new();
    out.push_str(name);
    out.push('(');
    render_fields(&params.fields, &mut out, paren_desc);
    out.push(')');
    if params.closed {
        out.push('!');
    }
    if let Some(desc) = desc {
        out.push_str(" - ");
        if is_bare_tool_desc(desc) {
            out.push_str(desc);
        } else {
            out.push_str(&quote(desc));
        }
    }
    out
}

/// A tool description is written bare when it reads back unambiguously as "rest of the line".
fn is_bare_tool_desc(desc: &str) -> bool {
    !desc.is_empty()
        && !desc.starts_with('"')
        && desc.trim() == desc
        && !desc.contains(['\n', '\r'])
}

pub(crate) fn quote(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

fn render_fields(fields: &[Field], out: &mut String, paren_desc: bool) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&f.name);
        if !f.required {
            out.push('?');
        }
        out.push(':');
        render_node(&f.node, out, paren_desc);
    }
}

fn render_node(node: &Node, out: &mut String, paren_desc: bool) {
    let mut format = None;
    match &node.ty {
        Ty::Str(f) => {
            out.push_str("str");
            format = f.as_deref();
        }
        Ty::Int => out.push_str("int"),
        Ty::Num => out.push_str("num"),
        Ty::Bool => out.push_str("bool"),
        Ty::DateTime => out.push_str("datetime"),
        Ty::Date => out.push_str("date"),
        Ty::Enum(values) => {
            out.push_str("enum ");
            out.push_str(&values.join("|"));
        }
        Ty::Array(inner) => {
            out.push('[');
            render_node(inner, out, paren_desc);
            out.push(']');
        }
        Ty::Object(o) => {
            out.push('{');
            render_fields(&o.fields, out, paren_desc);
            out.push('}');
            if o.closed {
                out.push('!');
            }
        }
    }
    let anns = annotations(format, &node.cons);
    if !anns.is_empty() {
        out.push('<');
        out.push_str(&anns.join(","));
        out.push('>');
    }
    if let Some(desc) = &node.desc {
        if paren_desc && is_paren_desc(desc) {
            out.push('(');
            out.push_str(desc);
            out.push(')');
        } else {
            out.push(' ');
            out.push_str(&quote(desc));
        }
    }
}

/// A parameter description can be written as `(text)` when it reads back exactly: no
/// parentheses or line breaks inside, and not empty. This costs fewer tokens than a JSON string
/// inside a JSON request body; anything else stays JSON-quoted.
fn is_paren_desc(desc: &str) -> bool {
    !desc.is_empty() && !desc.contains(['(', ')', '\n', '\r'])
}

/// Annotations in a fixed order, so rendering is deterministic.
fn annotations(format: Option<&str>, c: &Cons) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(f) = format {
        a.push(f.to_string());
    }
    let num = |k: &str, n: &Option<Number>, a: &mut Vec<String>| {
        if let Some(n) = n {
            a.push(format!("{k}={n}"));
        }
    };
    num("min", &c.minimum, &mut a);
    num("max", &c.maximum, &mut a);
    num("gt", &c.exclusive_minimum, &mut a);
    num("lt", &c.exclusive_maximum, &mut a);
    let count = |k: &str, n: Option<u64>, a: &mut Vec<String>| {
        if let Some(n) = n {
            a.push(format!("{k}={n}"));
        }
    };
    count("minlen", c.min_length, &mut a);
    count("maxlen", c.max_length, &mut a);
    count("minitems", c.min_items, &mut a);
    count("maxitems", c.max_items, &mut a);
    if c.unique_items {
        a.push("unique".into());
    }
    a
}

/// Parse one tool line. The whole line must be consumed.
pub(crate) fn parse(line: &str) -> Result<Signature, String> {
    let mut c = Cursor::new(line);
    let name = c.take_while(is_name_char);
    if name.is_empty() {
        return Err(at(&c, "expected tool name"));
    }
    if !c.eat("(") {
        return Err(at(&c, "expected '('"));
    }
    let fields = parse_fields(&mut c, ')', 1)?;
    let closed = c.eat("!");
    let desc = if c.is_empty() {
        None
    } else if c.eat(" - ") {
        if c.peek() == Some('"') {
            let d = parse_quoted(&mut c)?;
            if !c.is_empty() {
                return Err(at(&c, "trailing text after quoted description"));
            }
            Some(d)
        } else if c.is_empty() {
            return Err(at(&c, "empty bare description"));
        } else {
            Some(c.rest().to_string())
        }
    } else {
        return Err(at(&c, "expected ' - ' or end of line"));
    };
    Ok(Signature {
        name: name.to_string(),
        desc,
        params: Obj { fields, closed },
    })
}

fn at(c: &Cursor<'_>, msg: &str) -> String {
    format!("{msg} at byte {}", c.pos())
}

/// Fields up to and including `close`. `depth` is the depth of the fields' own nodes; the depth
/// check lives in [`parse_node`] only, mirroring `schema::node_from_schema`.
fn parse_fields(c: &mut Cursor<'_>, close: char, depth: usize) -> Result<Vec<Field>, String> {
    let close_str = close.to_string();
    let mut fields: Vec<Field> = Vec::new();
    c.skip_ws();
    if c.eat(&close_str) {
        return Ok(fields);
    }
    loop {
        let name = c.take_while(is_name_char);
        if name.is_empty() {
            return Err(at(c, "expected field name"));
        }
        if fields.iter().any(|f| f.name == name) {
            return Err(at(c, &format!("duplicate field '{name}'")));
        }
        let required = !c.eat("?");
        if !c.eat(":") {
            return Err(at(c, "expected ':'"));
        }
        let node = parse_node(c, depth)?;
        fields.push(Field {
            name: name.to_string(),
            required,
            node,
        });
        c.skip_ws();
        if c.eat(",") {
            c.skip_ws();
            continue;
        }
        if c.eat(&close_str) {
            return Ok(fields);
        }
        return Err(at(c, &format!("expected ',' or '{close}'")));
    }
}

/// A node at `depth` (top-level fields are depth 1; array items and object fields are one
/// deeper than their parent). Same rule as the schema side, so an empty object at exactly
/// [`MAX_DEPTH`] is accepted by both and anything deeper by neither.
fn parse_node(c: &mut Cursor<'_>, depth: usize) -> Result<Node, String> {
    if depth > MAX_DEPTH {
        return Err(at(c, "nesting too deep"));
    }
    let mut ty = if c.eat("[") {
        let inner = parse_node(c, depth + 1)?;
        c.skip_ws();
        if !c.eat("]") {
            return Err(at(c, "expected ']'"));
        }
        Ty::Array(Box::new(inner))
    } else if c.eat("{") {
        let fields = parse_fields(c, '}', depth + 1)?;
        Ty::Object(Obj {
            fields,
            closed: c.eat("!"),
        })
    } else if c.eat("enum ") {
        let mut values: Vec<String> = Vec::new();
        loop {
            let v = c.take_while(is_enum_char);
            if v.is_empty() {
                return Err(at(c, "expected enum value"));
            }
            if values.iter().any(|x| x == v) {
                return Err(at(c, &format!("duplicate enum value '{v}'")));
            }
            values.push(v.to_string());
            if !c.eat("|") {
                break;
            }
        }
        Ty::Enum(values)
    } else {
        let word = c.take_while(|ch| ch.is_ascii_alphabetic());
        match word {
            "str" => Ty::Str(None),
            "int" => Ty::Int,
            "num" => Ty::Num,
            "bool" => Ty::Bool,
            "datetime" => Ty::DateTime,
            "date" => Ty::Date,
            _ => return Err(at(c, &format!("unknown type '{word}'"))),
        }
    };
    let cons = if c.eat("<") {
        parse_annotations(c, &mut ty)?
    } else {
        Cons::default()
    };
    // Optional description, in either form: `(text)` directly after the type, or whitespace then
    // a JSON string. Nothing else after a type starts with '(' or '"', so neither lookahead can
    // misfire.
    if c.eat("(") {
        let text = c.take_while(|ch| !matches!(ch, '(' | ')' | '\n' | '\r'));
        if text.is_empty() || !c.eat(")") {
            return Err(at(c, "bad (description)"));
        }
        return Ok(Node {
            ty,
            cons,
            desc: Some(text.to_string()),
        });
    }
    let mut probe = Cursor::new(c.rest());
    probe.skip_ws();
    let desc = if probe.peek() == Some('"') {
        c.skip_ws();
        Some(parse_quoted(c)?)
    } else {
        None
    };
    Ok(Node { ty, cons, desc })
}

/// `<ann,ann>` after a base type; the opening `<` is already consumed. Each annotation must fit
/// the type it follows, so a parsed signature always maps to a schema the encoder accepts.
fn parse_annotations(c: &mut Cursor<'_>, ty: &mut Ty) -> Result<Cons, String> {
    let is_str = matches!(ty, Ty::Str(_) | Ty::DateTime | Ty::Date);
    let is_num = matches!(ty, Ty::Int | Ty::Num);
    let is_arr = matches!(ty, Ty::Array(_));
    let mut cons = Cons::default();
    loop {
        let key = c.take_while(is_name_char);
        if c.eat("=") {
            match key {
                "min" | "max" | "gt" | "lt" => {
                    fits(c, key, is_num)?;
                    let n = parse_number(c)?;
                    let slot = match key {
                        "min" => &mut cons.minimum,
                        "max" => &mut cons.maximum,
                        "gt" => &mut cons.exclusive_minimum,
                        _ => &mut cons.exclusive_maximum,
                    };
                    set_once(slot, n).map_err(|()| at(c, &format!("repeated '{key}'")))?;
                }
                "minlen" | "maxlen" | "minitems" | "maxitems" => {
                    fits(c, key, if key.ends_with("len") { is_str } else { is_arr })?;
                    let n = parse_number(c)?
                        .as_u64()
                        .ok_or_else(|| at(c, &format!("'{key}' needs a non-negative integer")))?;
                    let slot = match key {
                        "minlen" => &mut cons.min_length,
                        "maxlen" => &mut cons.max_length,
                        "minitems" => &mut cons.min_items,
                        _ => &mut cons.max_items,
                    };
                    set_once(slot, n).map_err(|()| at(c, &format!("repeated '{key}'")))?;
                }
                _ => return Err(at(c, &format!("unknown annotation '{key}'"))),
            }
        } else if is_arr && key == "unique" && !cons.unique_items {
            cons.unique_items = true;
        } else if let Ty::Str(format @ None) = ty
            && !key.is_empty()
        {
            *format = Some(key.to_string());
        } else {
            return Err(at(c, &format!("unexpected annotation '{key}'")));
        }
        if c.eat(">") {
            return Ok(cons);
        }
        if !c.eat(",") {
            return Err(at(c, "expected ',' or '>'"));
        }
    }
}

fn fits(c: &Cursor<'_>, key: &str, ok: bool) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(at(c, &format!("'{key}' does not apply here")))
    }
}

fn set_once<T>(slot: &mut Option<T>, v: T) -> Result<(), ()> {
    if slot.is_some() {
        return Err(());
    }
    *slot = Some(v);
    Ok(())
}

/// A JSON number. Taken by character class first: serde_json's stream reader rejects a number
/// followed by `>` or `,`-less text, so it cannot be used directly here.
fn parse_number(c: &mut Cursor<'_>) -> Result<Number, String> {
    let tok = c.take_while(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E'));
    serde_json::from_str::<Number>(tok).map_err(|e| at(c, &format!("bad number '{tok}': {e}")))
}

/// A JSON string literal, via serde_json so every escape is handled exactly as it was written.
pub(crate) fn parse_quoted(c: &mut Cursor<'_>) -> Result<String, String> {
    let mut stream = serde_json::Deserializer::from_str(c.rest()).into_iter::<String>();
    match stream.next() {
        Some(Ok(s)) => {
            let used = stream.byte_offset();
            if !c.advance(used) {
                return Err(at(c, "bad string boundary"));
            }
            Ok(s)
        }
        Some(Err(e)) => Err(at(c, &format!("bad quoted string: {e}"))),
        None => Err(at(c, "expected quoted string")),
    }
}
