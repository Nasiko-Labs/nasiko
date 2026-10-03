use serde_json::Value;

use crate::types::{ToolCall, ToolDef};
use crate::validate::{ValidationError, validate_arguments};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("unknown_tool: {0}")]
    UnknownTool(String),
    #[error("invalid_arguments: {0}")]
    InvalidArguments(String),
    #[error("malformed_call: {0}")]
    MalformedCall(String),
}

impl DecodeError {
    pub fn error_code(&self) -> &'static str {
        match self {
            DecodeError::UnknownTool(_) => "unknown_tool",
            DecodeError::InvalidArguments(_) => "invalid_arguments",
            DecodeError::MalformedCall(_) => "invalid_arguments",
        }
    }
}

impl From<ValidationError> for DecodeError {
    fn from(err: ValidationError) -> Self {
        DecodeError::InvalidArguments(err.to_string())
    }
}

/// Decode model output text into standard ToolCall instances.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut calls = Vec::new();
    let mut search_idx = 0;
    let mut call_counter = 1;

    while let Some(start_offset) = text[search_idx..].find("<<call") {
        let call_start = search_idx + start_offset;
        let after_marker = call_start + "<<call".len();

        if after_marker >= text.len() {
            return Err(DecodeError::MalformedCall("incomplete call marker".to_string()));
        }

        // Must have whitespace after <<call
        let rest = &text[after_marker..];
        let trimmed_leading = rest.trim_start();
        if trimmed_leading.len() == rest.len() {
            // No whitespace after <<call
            search_idx = after_marker;
            continue;
        }

        // Extract tool name until whitespace or '{'
        let name_end = match trimmed_leading.find(|c: char| c.is_whitespace() || c == '{') {
            Some(idx) => idx,
            None => {
                return Err(DecodeError::MalformedCall("missing tool arguments".to_string()));
            }
        };

        let tool_name = trimmed_leading[..name_end].trim();
        if tool_name.is_empty() {
            return Err(DecodeError::MalformedCall("missing tool name".to_string()));
        }

        // Check if tool name exists in tools
        let tool_def = match tools.iter().find(|t| t.function.name == tool_name) {
            Some(td) => td,
            None => {
                return Err(DecodeError::UnknownTool(tool_name.to_string()));
            }
        };

        // Find JSON arguments starting at '{'
        let json_start_rel = match trimmed_leading[name_end..].find('{') {
            Some(idx) => name_end + idx,
            None => {
                return Err(DecodeError::MalformedCall(format!(
                    "missing JSON arguments for tool '{}'",
                    tool_name
                )));
            }
        };

        let json_start_abs = (text.len() - rest.len()) + (rest.len() - trimmed_leading.len()) + json_start_rel;

        // Parse balanced JSON object handling strings and escapes
        let (json_end_abs, end_marker_abs) = match find_balanced_json_and_closing(&text[json_start_abs..]) {
            Some((j_end, m_end)) => (json_start_abs + j_end, json_start_abs + m_end),
            None => {
                return Err(DecodeError::MalformedCall(format!(
                    "unclosed call marker or invalid JSON for tool '{}'",
                    tool_name
                )));
            }
        };

        let json_str = &text[json_start_abs..json_end_abs];
        let parsed_val: Value = serde_json::from_str(json_str).map_err(|e| {
            DecodeError::InvalidArguments(format!("JSON parse error: {}", e))
        })?;

        // Validate arguments against tool schema
        validate_arguments(&parsed_val, tool_def.function.parameters.as_ref())?;

        calls.push(ToolCall::new(
            format!("call_{}", call_counter),
            tool_name,
            json_str,
        ));
        call_counter += 1;
        search_idx = end_marker_abs;
    }

    Ok(calls)
}

/// Helper that locates the end of a balanced JSON object and its following closing `>>`.
/// Returns `Some((json_end_offset, marker_end_offset))` relative to input slice `s`.
pub fn find_balanced_json_and_closing(s: &str) -> Option<(usize, usize)> {
    if !s.starts_with('{') {
        return None;
    }

    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut json_end = None;

    for (idx, ch) in s.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    json_end = Some(idx + ch.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }

    let j_end = json_end?;
    let after_json = &s[j_end..];
    let trimmed = after_json.trim_start();
    if !trimmed.starts_with(">>") {
        return None;
    }

    let marker_offset = j_end + (after_json.len() - trimmed.len()) + 2;
    Some((j_end, marker_offset))
}
