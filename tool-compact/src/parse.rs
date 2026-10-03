//! Definition text → typed tree, the exact inverse of the renderer in `encode.rs`.
//!
//! [`decode_tools`] reads **only** the rendered text, so a successful round trip proves the
//! schema information survived compaction.

use serde_json::{Number, Value};

use crate::encode::innermost_object;
use crate::error::ParseError;
use crate::schema::{
    Format, Prop, Tool, Ty, collapse, enum_kind, is_bare_enum_value, is_key, is_name,
};
use crate::types::{CompactTools, ToolDef};

/// Rebuild the tool schemas (canonical form, see [`crate::normalize`]) from the definitions.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>, ParseError> {
    parse_definitions(&compact.definitions)
        .map(|tools| tools.iter().map(crate::schema::to_tooldef).collect())
}

pub(crate) fn parse_definitions(text: &str) -> Result<Vec<Tool>, ParseError> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    let mut tools = Vec::new();
    while i < lines.len() {
        let line = lines.get(i).copied().unwrap_or_default();
        let Some(header) = line.strip_prefix("## ") else {
            return Err(err(i, "expected a tool header (## name)"));
        };
        let (name, desc) = match header.split_once(": ") {
            Some((n, d)) => (n, Some(d)),
            None => (header, None),
        };
        if !is_name(name) {
            return Err(err(i, "invalid tool name"));
        }
        let desc = match desc {
            Some(d) => match collapse(d) {
                Some(c) if c == d => Some(c),
                _ => return Err(err(i, "tool description is empty or not normalized")),
            },
            None => None,
        };
        i += 1;
        let params = parse_props(&lines, &mut i, 0)?;
        tools.push(Tool {
            name: name.to_string(),
            desc,
            params,
        });
    }
    Ok(tools)
}

fn err(line: usize, reason: &str) -> ParseError {
    ParseError {
        line: line + 1,
        reason: reason.to_string(),
    }
}

/// Parse the property lines at exactly `depth` (two spaces per level), recursing into children.
fn parse_props(lines: &[&str], i: &mut usize, depth: usize) -> Result<Vec<Prop>, ParseError> {
    let mut props = Vec::new();
    while let Some(line) = lines.get(*i).copied() {
        if line.starts_with("## ") {
            break;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent < depth * 2 {
            break;
        }
        if indent != depth * 2 {
            return Err(err(*i, "unexpected indentation"));
        }
        let body = line.trim_start_matches(' ');
        let mut prop = parse_prop_line(body).map_err(|r| err(*i, &r))?;
        if props.iter().any(|p: &Prop| p.key == prop.key) {
            return Err(err(*i, "duplicate property"));
        }
        *i += 1;
        if innermost_object(&prop.ty).is_some() {
            let children = parse_props(lines, i, depth + 1)?;
            set_innermost_object(&mut prop.ty, children);
        }
        props.push(prop);
    }
    Ok(props)
}

fn set_innermost_object(ty: &mut Ty, children: Vec<Prop>) {
    match ty {
        Ty::Obj(props) => *props = children,
        Ty::Array(inner) => set_innermost_object(inner, children),
        _ => {}
    }
}

/// `key?: type[ range][ = default][ # description]`
fn parse_prop_line(line: &str) -> Result<Prop, String> {
    let (key_part, rest) = line.split_once(": ").ok_or("expected 'key: type'")?;
    let (key, required) = match key_part.strip_suffix('?') {
        Some(k) => (k, false),
        None => (key_part, true),
    };
    if !is_key(key) {
        return Err(format!("invalid property name {key:?}"));
    }
    let (ty, mut rest) = parse_ty(rest)?;

    let mut min = None;
    let mut max = None;
    if let Some(after) = rest.strip_prefix(' ')
        && after.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')
    {
        if !matches!(ty, Ty::Int | Ty::Num) {
            return Err("range on a non-numeric type".into());
        }
        let range = after.split(' ').next().unwrap_or_default();
        let (lo, hi) = range.split_once("..").ok_or("invalid range")?;
        min = parse_bound(lo)?;
        max = parse_bound(hi)?;
        if min.is_none() && max.is_none() {
            return Err("empty range".into());
        }
        rest = after.get(range.len()..).unwrap_or_default();
    }

    let mut default = None;
    if let Some(after) = rest.strip_prefix(" = ") {
        let len = scalar_len(after).ok_or("invalid default")?;
        let raw = after.get(..len).ok_or("invalid default")?;
        let value: Value = serde_json::from_str(raw).map_err(|_| "invalid default")?;
        if value.is_array() || value.is_object() {
            return Err("default must be a scalar".into());
        }
        default = Some(value);
        rest = after.get(len..).unwrap_or_default();
    }

    let mut desc = None;
    if let Some(after) = rest.strip_prefix(" # ") {
        match collapse(after) {
            Some(c) if c == after => desc = Some(c),
            _ => return Err("description is empty or not normalized".into()),
        }
        rest = "";
    }
    if !rest.is_empty() {
        return Err(format!("unexpected trailing text {rest:?}"));
    }
    Ok(Prop {
        key: key.to_string(),
        required,
        ty,
        desc,
        default,
        min,
        max,
    })
}

fn parse_bound(s: &str) -> Result<Option<Number>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Number>(s)
        .map(Some)
        .map_err(|_| format!("invalid number {s:?}"))
}

/// Parse a type at the start of `s`; returns it and the unparsed remainder.
fn parse_ty(s: &str) -> Result<(Ty, &str), String> {
    if let Some(inner) = s.strip_prefix('[') {
        let (ty, rest) = parse_ty(inner)?;
        let rest = rest.strip_prefix(']').ok_or("expected ']'")?;
        return Ok((Ty::Array(Box::new(ty)), rest));
    }
    if let Some(after) = s.strip_prefix("enum")
        && after.starts_with('[')
    {
        let len = json_len(after).ok_or("unterminated enum array")?;
        let raw = after.get(..len).ok_or("invalid enum")?;
        let values: Vec<Value> = serde_json::from_str(raw).map_err(|_| "invalid enum array")?;
        if enum_kind(&values).is_none() {
            return Err("enum values must be all strings or all numbers".into());
        }
        return Ok((Ty::Enum(values), after.get(len..).unwrap_or_default()));
    }
    let end = s.find([' ', ']']).unwrap_or(s.len());
    let token = s.get(..end).unwrap_or_default();
    let rest = s.get(end..).unwrap_or_default();
    let ty = match token {
        "str" => Ty::Str(None),
        "int" => Ty::Int,
        "num" => Ty::Num,
        "bool" => Ty::Bool,
        "null" => Ty::Null,
        "obj" => Ty::Obj(Vec::new()),
        _ => {
            if let Some(f) = Format::ALL.into_iter().find(|f| f.keyword() == token) {
                Ty::Str(Some(f))
            } else {
                let values: Vec<&str> = token.split('|').collect();
                if !values.iter().all(|v| is_bare_enum_value(v)) {
                    return Err(format!("unknown type {token:?}"));
                }
                Ty::Enum(
                    values
                        .into_iter()
                        .map(|v| Value::String(v.to_string()))
                        .collect(),
                )
            }
        }
    };
    Ok((ty, rest))
}

/// Byte length of the JSON scalar at the start of `s` (a string literal, or a bare token up to
/// the next space).
fn scalar_len(s: &str) -> Option<usize> {
    if s.starts_with('"') {
        json_len(s)
    } else {
        Some(s.find(' ').unwrap_or(s.len()))
    }
}

/// Byte length of the JSON string / array / object at the start of `s`, string- and
/// escape-aware.
pub(crate) fn json_len(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_definitions_are_parse_errors() {
        for bad in [
            "title: str",
            "## t\ntitle str",
            "## t\ntitle: strr x",
            "## t\n   title: str",
            "## t\ntitle: str 1..2",
            "## t\ntitle: int ..",
            "## t\ntitle: str = [1]",
            "## t\ntitle: str #  two  spaces",
            "## t\ntitle: enum[\"a\",1]",
            "## t\ntitle: [str",
            "## t\na: str\na: int",
            "## bad name\n",
        ] {
            assert!(parse_definitions(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn json_len_is_string_aware() {
        assert_eq!(json_len(r#"["a]",1] tail"#), Some(8));
        assert_eq!(json_len(r#""x\"y" z"#), Some(6));
        assert_eq!(json_len("[1,2"), None);
    }
}
