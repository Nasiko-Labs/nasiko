use crate::error::CompactError;
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_arguments;
use serde_json::Value;

pub(crate) const CALL_OPEN: &str = "<<call ";
pub(crate) const CALL_CLOSE: &str = ">>";

pub(crate) struct RawCall {
    pub name: String,
    pub args: Value,
}

pub(crate) enum Parsed {
    /// A complete call, ending at byte offset `end`.
    Call { call: RawCall, end: usize },
    /// A prefix of a call; more input is needed.
    Incomplete,
    /// Not a call at all; the `<<` is ordinary text.
    NotACall,
}

/// Find the end of the JSON object starting at `start`, tracking string and
/// escape state. This is what makes a `>>` inside a string argument safe: the
/// terminator is never searched for textually.
pub(crate) fn find_json_end(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (off, c) in s[start..].char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if in_str => esc = true,
            '"' => in_str = !in_str,
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + off + c.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn skip_spaces(s: &str, mut i: usize) -> usize {
    while i < s.len() && s[i..].starts_with([' ', '\t', '\n', '\r']) {
        i += 1;
    }
    i
}

/// Attempt to parse a call beginning at `start`, which points at a `<<`.
pub(crate) fn parse_call_at(s: &str, start: usize) -> Result<Parsed, CompactError> {
    let rest = &s[start..];
    if !rest.starts_with(CALL_OPEN) {
        // A prefix of the opening marker means we are mid-stream.
        if CALL_OPEN.starts_with(rest) {
            return Ok(Parsed::Incomplete);
        }
        return Ok(Parsed::NotACall);
    }

    let mut i = skip_spaces(s, start + CALL_OPEN.len());
    let name_start = i;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    if i >= s.len() {
        return Ok(Parsed::Incomplete);
    }
    let name = s[name_start..i].to_string();
    if name.is_empty() {
        return Err(CompactError::MalformedCall { reason: "missing tool name".into() });
    }

    i = skip_spaces(s, i);
    if i >= s.len() {
        return Ok(Parsed::Incomplete);
    }
    if !s[i..].starts_with('{') {
        return Err(CompactError::MalformedCall {
            reason: format!("expected `{{` after tool name `{name}`"),
        });
    }

    let Some(json_end) = find_json_end(s, i) else {
        return Ok(Parsed::Incomplete);
    };
    let args: Value = serde_json::from_str(&s[i..json_end]).map_err(|e| {
        CompactError::InvalidArguments { tool: name.clone(), reason: format!("invalid JSON: {e}") }
    })?;

    let j = skip_spaces(s, json_end);
    if j >= s.len() {
        return Ok(Parsed::Incomplete);
    }
    if !s[j..].starts_with(CALL_CLOSE) {
        if CALL_CLOSE.starts_with(&s[j..]) {
            return Ok(Parsed::Incomplete);
        }
        return Err(CompactError::MalformedCall {
            reason: format!("expected `{CALL_CLOSE}` after arguments for `{name}`"),
        });
    }

    Ok(Parsed::Call { call: RawCall { name, args }, end: j + CALL_CLOSE.len() })
}

pub(crate) fn build_call(raw: RawCall, tools: &[ToolDef]) -> Result<ToolCall, CompactError> {
    let def = tools
        .iter()
        .find(|t| t.name == raw.name)
        .ok_or_else(|| CompactError::UnknownTool { name: raw.name.clone() })?;
    validate_arguments(&raw.args, def)?;
    let arguments = serde_json::to_string(&raw.args).map_err(|e| CompactError::InvalidArguments {
        tool: raw.name.clone(),
        reason: e.to_string(),
    })?;
    Ok(ToolCall { name: raw.name, arguments })
}

/// Decode every complete call in `text`, validating each against its schema.
///
/// Text before, between and after calls is ignored. A plain answer with no call
/// yields an empty vector. A trailing partial call is ignored rather than
/// guessed at.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = text[pos..].find("<<") {
        let abs = pos + rel;
        match parse_call_at(text, abs)? {
            Parsed::Call { call, end } => {
                out.push(build_call(call, tools)?);
                pos = end;
            }
            Parsed::Incomplete => break,
            Parsed::NotACall => pos = abs + 2,
        }
    }
    Ok(out)
}
