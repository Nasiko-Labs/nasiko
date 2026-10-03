//! Full-text deterministic state machine decoder for compact tool calling.

use serde_json::Value;

use crate::error::CompactToolError;
use crate::types::{DecodeOutput, ToolCall, ToolDefinition};
use crate::validator::validate_tool_call;

/// Decodes all tool calls from model output text and validates them against `tools`.
///
/// Returns `Ok(Vec<ToolCall>)` on success, or `Err(CompactToolError)` on any syntax,
/// malformed JSON, unknown tool, type mismatch, or unclosed marker error.
pub fn decode_calls(
    text: &str,
    tools: &[ToolDefinition],
) -> Result<Vec<ToolCall>, CompactToolError> {
    let output = decode_output(text, tools)?;
    Ok(output.tool_calls)
}

/// Decodes both conversational text and tool calls from model output text.
pub fn decode_output(
    text: &str,
    tools: &[ToolDefinition],
) -> Result<DecodeOutput, CompactToolError> {
    let mut text_out = String::new();
    let mut tool_calls = Vec::new();
    let mut call_index = 0;

    let mut state = ParserState::Text;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match &mut state {
            ParserState::Text => {
                if ch == '<' {
                    state = ParserState::PotentialOpen1;
                } else {
                    text_out.push(ch);
                }
            }
            ParserState::PotentialOpen1 => {
                if ch == '<' {
                    state = ParserState::CallKeyword { matched: 0 };
                } else {
                    text_out.push('<');
                    text_out.push(ch);
                    state = ParserState::Text;
                }
            }
            ParserState::CallKeyword { matched } => {
                let target = ['c', 'a', 'l', 'l'];
                if *matched < target.len() {
                    if ch == target[*matched] {
                        *matched += 1;
                    } else {
                        // Mismatch during "call" keyword; flush as text
                        text_out.push_str("<<");
                        for &c in &target[..*matched] {
                            text_out.push(c);
                        }
                        text_out.push(ch);
                        state = ParserState::Text;
                    }
                } else {
                    // All 4 characters 'c','a','l','l' matched. Next must be whitespace.
                    if ch.is_whitespace() {
                        state = ParserState::ToolNamePre;
                    } else {
                        text_out.push_str("<<call");
                        text_out.push(ch);
                        state = ParserState::Text;
                    }
                }
            }
            ParserState::ToolNamePre => {
                if ch.is_whitespace() {
                    // Consume additional whitespace between 'call' and tool name
                } else if is_ident_start(ch) {
                    let mut name = String::new();
                    name.push(ch);
                    state = ParserState::ToolName { name };
                } else {
                    text_out.push_str("<<call ");
                    text_out.push(ch);
                    state = ParserState::Text;
                }
            }
            ParserState::ToolName { name } => {
                if is_ident_continue(ch) {
                    name.push(ch);
                } else if ch.is_whitespace() {
                    let tool_name = name.clone();
                    state = ParserState::ToolArgsPre { tool_name };
                } else if ch == '{' {
                    let tool_name = name.clone();
                    let mut args_buf = String::new();
                    args_buf.push('{');
                    state = ParserState::JsonScan {
                        tool_name,
                        args_buf,
                        depth: 1,
                        in_string: false,
                        escaped: false,
                    };
                } else {
                    text_out.push_str("<<call ");
                    text_out.push_str(name);
                    text_out.push(ch);
                    state = ParserState::Text;
                }
            }
            ParserState::ToolArgsPre { tool_name } => {
                if ch.is_whitespace() {
                    // Consume whitespace before '{'
                } else if ch == '{' {
                    let mut args_buf = String::new();
                    args_buf.push('{');
                    state = ParserState::JsonScan {
                        tool_name: tool_name.clone(),
                        args_buf,
                        depth: 1,
                        in_string: false,
                        escaped: false,
                    };
                } else {
                    text_out.push_str("<<call ");
                    text_out.push_str(tool_name);
                    text_out.push(' ');
                    text_out.push(ch);
                    state = ParserState::Text;
                }
            }
            ParserState::JsonScan {
                tool_name,
                args_buf,
                depth,
                in_string,
                escaped,
            } => {
                if *in_string {
                    args_buf.push(ch);
                    if *escaped {
                        *escaped = false;
                    } else if ch == '\\' {
                        *escaped = true;
                    } else if ch == '"' {
                        *in_string = false;
                    }
                    // Literal '>' inside string is simply accumulated
                } else {
                    if ch == '"' {
                        *in_string = true;
                        args_buf.push(ch);
                    } else if ch == '{' {
                        *depth += 1;
                        args_buf.push(ch);
                    } else if ch == '}' {
                        *depth = depth.saturating_sub(1);
                        args_buf.push(ch);
                    } else if *depth == 0 && ch == '>' {
                        state = ParserState::PotentialClose1 {
                            tool_name: tool_name.clone(),
                            args_buf: args_buf.clone(),
                        };
                    } else {
                        args_buf.push(ch);
                    }
                }
            }
            ParserState::PotentialClose1 {
                tool_name,
                args_buf,
            } => {
                if ch == '>' {
                    // Tool call successfully closed with '>>'
                    let raw_args = args_buf.trim();
                    let parsed_json: Value = serde_json::from_str(raw_args).map_err(|e| {
                        CompactToolError::MalformedJson {
                            tool_name: tool_name.clone(),
                            details: e.to_string(),
                        }
                    })?;

                    if !parsed_json.is_object() {
                        return Err(CompactToolError::InvalidFieldType {
                            tool_name: tool_name.clone(),
                            field: "arguments".to_string(),
                            expected: "object",
                            found: "non-object".to_string(),
                        });
                    }

                    validate_tool_call(tool_name, &parsed_json, tools)?;

                    let call_id = format!("call_{}_{}", call_index, tool_name);
                    let canonical_arguments = serde_json::to_string(&parsed_json)
                        .unwrap_or_else(|_| raw_args.to_string());

                    tool_calls.push(ToolCall {
                        id: call_id,
                        name: tool_name.clone(),
                        arguments: canonical_arguments,
                    });

                    call_index += 1;
                    state = ParserState::Text;
                } else {
                    // Not a closing '>>'; revert to JsonScan
                    let mut new_buf = args_buf.clone();
                    new_buf.push('>');
                    new_buf.push(ch);
                    state = ParserState::JsonScan {
                        tool_name: tool_name.clone(),
                        args_buf: new_buf,
                        depth: 0,
                        in_string: false,
                        escaped: false,
                    };
                }
            }
        }
    }

    // Process terminal state at end-of-text
    match state {
        ParserState::Text => Ok(DecodeOutput {
            text: text_out,
            tool_calls,
        }),
        ParserState::PotentialOpen1 => {
            text_out.push('<');
            Ok(DecodeOutput {
                text: text_out,
                tool_calls,
            })
        }
        ParserState::CallKeyword { matched } => {
            text_out.push_str("<<");
            let target = ['c', 'a', 'l', 'l'];
            for &c in &target[..matched] {
                text_out.push(c);
            }
            Ok(DecodeOutput {
                text: text_out,
                tool_calls,
            })
        }
        ParserState::ToolNamePre
        | ParserState::ToolName { .. }
        | ParserState::ToolArgsPre { .. }
        | ParserState::JsonScan { .. }
        | ParserState::PotentialClose1 { .. } => Err(CompactToolError::UnclosedCallMarker),
    }
}

#[derive(Debug, Clone)]
enum ParserState {
    Text,
    PotentialOpen1,
    CallKeyword {
        matched: usize,
    },
    ToolNamePre,
    ToolName {
        name: String,
    },
    ToolArgsPre {
        tool_name: String,
    },
    JsonScan {
        tool_name: String,
        args_buf: String,
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    PotentialClose1 {
        tool_name: String,
        args_buf: String,
    },
}

#[inline]
fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

#[inline]
fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}
