//! [`decode_calls`]: extract `<<call …>>` markers from model text, fail-closed.

use crate::schema::CompactSchema;
use crate::types::{Error, ToolCall, ToolDef};

/// Decode every `<<call name {json}>>` marker in `text`.
///
/// * Markers may be surrounded by plain text; several may appear in one message.
/// * Text with no marker decodes to an empty vec (a plain answer).
/// * Fail-closed: the first unknown tool, schema violation, or malformed marker
///   aborts the whole decode with [`Error`]; no partial calls are returned.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, Error> {
    let schemas = build_schemas(tools)?;
    let mut calls = Vec::new();
    let mut rest = text;
    loop {
        let Some(idx) = find_marker_start(rest) else {
            break;
        };
        match parse_one_marker(&rest[idx..], &schemas)? {
            Some((call, consumed)) => {
                calls.push(call);
                rest = &rest[idx + consumed..];
            }
            // Non-streaming: an incomplete marker at end of input is malformed.
            None => {
                return Err(Error::Malformed("unterminated <<call marker".to_string()));
            }
        }
    }
    // A dangling "<<call" that never formed a marker is malformed output.
    if rest.contains("<<call") {
        return Err(Error::Malformed("unterminated <<call marker".to_string()));
    }
    Ok(calls)
}

pub(crate) fn build_schemas(tools: &[ToolDef]) -> Result<Vec<(String, CompactSchema)>, Error> {
    tools
        .iter()
        .map(|t| {
            let schema = match t.parameters.as_ref() {
                None => CompactSchema { fields: Vec::new() },
                Some(s) => CompactSchema::from_json_schema(&t.name, s).map_err(|_| {
                    Error::UnsupportedSchema {
                        tool: t.name.clone(),
                        reason: "tool schema not supported for decoding".to_string(),
                    }
                })?,
            };
            Ok((t.name.clone(), schema))
        })
        .collect()
}

/// Byte index of `<<call` when followed by ASCII whitespace, else `None`.
pub(crate) fn find_marker_start(text: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find("<<call") {
        let abs = from + i;
        if text[abs + 6..].starts_with(|c: char| c.is_ascii_whitespace()) {
            return Some(abs);
        }
        from = abs + 6;
    }
    None
}

/// Parse one marker at the start of `text` (which must begin with `<<call `).
///
/// Returns `Ok(None)` when the input ends mid-marker — the caller should feed more
/// input (streaming) or report [`Error::Malformed`] (one-shot). Returns the call and
/// the bytes consumed through the closing `>>` otherwise.
pub(crate) fn parse_one_marker(
    text: &str,
    schemas: &[(String, CompactSchema)],
) -> Result<Option<(ToolCall, usize)>, Error> {
    debug_assert!(text.starts_with("<<call"));
    let bytes = text.as_bytes();
    let mut pos = "<<call".len();

    // Tool name: up to the next whitespace.
    pos = skip_ws(text, pos);
    if pos >= bytes.len() {
        return Ok(None);
    }
    let name_start = pos;
    while pos < bytes.len() && !bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= bytes.len() {
        return Ok(None); // name may continue in the next chunk
    }
    let name = &text[name_start..pos];
    if name.is_empty() {
        return Err(Error::Malformed(
            "missing tool name in <<call marker".to_string(),
        ));
    }

    // JSON object.
    pos = skip_ws(text, pos);
    if pos >= bytes.len() {
        return Ok(None);
    }
    if bytes[pos] != b'{' {
        return Err(Error::Malformed(format!(
            "expected '{{' after tool name {name:?} in <<call marker"
        )));
    }
    let Some(json_end) = scan_json_object(text, pos) else {
        return Ok(None); // object not closed yet
    };
    let args_src = &text[pos..json_end];

    // Closing ">>" (whitespace allowed between "}" and ">>").
    pos = skip_ws(text, json_end);
    let tail = &text[pos..];
    if tail.starts_with(">>") {
        pos += 2;
    } else if tail == ">" {
        return Ok(None); // second ">" may arrive in the next chunk
    } else if tail.is_empty() {
        return Ok(None);
    } else {
        return Err(Error::Malformed(format!(
            "expected '>>' to close <<call {name} marker"
        )));
    }

    let args: serde_json::Value = serde_json::from_str(args_src)
        .map_err(|e| Error::Malformed(format!("arguments for {name:?} are not valid JSON: {e}")))?;

    let schema = schemas
        .iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| Error::UnknownTool(name.to_string()))?;
    schema.1.validate(name, &args)?;

    Ok(Some((ToolCall::new(name, args), pos)))
}

fn skip_ws(text: &str, mut pos: usize) -> usize {
    let bytes = text.as_bytes();
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    pos
}

/// Byte index just past the closing `}` of the JSON object starting at `start`
/// (which must be `{`), JSON-string aware so `>>` or braces inside strings — and
/// `\` escapes — do not end the scan. `None` when the object never closes.
fn scan_json_object(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            escape = false;
        } else if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}
