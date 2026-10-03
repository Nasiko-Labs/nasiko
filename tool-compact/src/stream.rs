use crate::{
    error::{DecodeError, EncodeError},
    schema::validate_call,
    types::{StreamEvent, ToolCall, ToolDef},
};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    error: Option<DecodeError>,
    next_index: usize,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Result<Self, EncodeError> {
        let mut set = std::collections::HashSet::new();
        for tool in tools {
            if !set.insert(tool.name.clone()) {
                return Err(EncodeError::DuplicateTool(tool.name.clone()));
            }
        }
        Ok(Self {
            tools: tools.to_vec(),
            buffer: String::new(),
            error: None,
            next_index: 0,
        })
    }

    pub fn push(&mut self, text: &str) -> Result<Vec<StreamEvent>, DecodeError> {
        if let Some(err) = self.error.clone() {
            return Err(err);
        }
        self.buffer.push_str(text);
        let mut events = Vec::new();
        while let Some((start, end)) = find_next_call(&self.buffer) {
            let Some(prefix) = self.buffer.get(..start).map(str::to_owned) else {
                break;
            };
            if !prefix.is_empty() {
                events.push(StreamEvent::Text(prefix));
            }
            let Some(call_text) = self.buffer.get(start..end) else {
                break;
            };
            let parsed = parse_call(call_text, &self.tools)?;
            events.push(StreamEvent::Call {
                index: self.next_index,
                call: parsed,
            });
            self.next_index += 1;
            if let Some(rest) = self.buffer.get(end..) {
                self.buffer = rest.to_string();
            } else {
                self.buffer.clear();
            }
        }
        let marker = "<<call";
        if let Some(start) = self.buffer.find(marker) {
            let prefix = self.buffer.get(..start).unwrap_or_default().to_string();
            if !prefix.is_empty() {
                events.push(StreamEvent::Text(prefix));
            }
            self.buffer.drain(..start);
            return Ok(events);
        }

        let partial_marker_len = (1..marker.len())
            .rev()
            .find(|length| self.buffer.ends_with(&marker[..*length]))
            .unwrap_or(0);
        let safe_end = self.buffer.len().saturating_sub(partial_marker_len);
        if safe_end > 0 {
            let text = self.buffer.get(..safe_end).unwrap_or_default().to_string();
            events.push(StreamEvent::Text(text));
            self.buffer.drain(..safe_end);
        }
        Ok(events)
    }

    pub fn finish(self) -> Result<Vec<StreamEvent>, DecodeError> {
        if let Some(err) = self.error.clone() {
            return Err(err);
        }
        let mut events = Vec::new();
        if self.buffer.contains("<<call") {
            return Err(DecodeError::MalformedCall {
                reason: "unterminated call".to_string(),
            });
        }
        if !self.buffer.is_empty() {
            events.push(StreamEvent::Text(self.buffer));
        }
        Ok(events)
    }
}

fn find_next_call(input: &str) -> Option<(usize, usize)> {
    let start = input.find("<<call")?;
    let body = input.get(start + 7..)?;
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0usize;
    let mut saw_open_brace = false;
    let bytes: Vec<char> = body.chars().collect();
    for (pos, ch) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            if *ch == '\\' {
                escaped = true;
                continue;
            }
            if *ch == '"' {
                in_string = false;
            }
            continue;
        }
        match *ch {
            '"' => {
                in_string = true;
            }
            '{' => {
                saw_open_brace = true;
                depth += 1;
            }
            '}' if saw_open_brace => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let rest = body.get(pos + 1..)?;
                    if rest.starts_with(">>") {
                        return Some((start, start + 7 + pos + 1 + 2));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_call(text: &str, tools: &[ToolDef]) -> Result<ToolCall, DecodeError> {
    let body = text.trim();
    if !body.starts_with("<<call") {
        return Err(DecodeError::MalformedCall {
            reason: "missing call marker".to_string(),
        });
    }
    let Some(after_call) = body.get(7..) else {
        return Err(DecodeError::MalformedCall {
            reason: "missing call body".to_string(),
        });
    };
    let trimmed = after_call.trim_start();
    let name_end = trimmed
        .find(char::is_whitespace)
        .or_else(|| trimmed.find('{'))
        .unwrap_or(trimmed.len());
    let Some(name) = trimmed.get(..name_end) else {
        return Err(DecodeError::MalformedCall {
            reason: "invalid tool name".to_string(),
        });
    };
    if name.is_empty() {
        return Err(DecodeError::MalformedCall {
            reason: "empty tool name".to_string(),
        });
    }
    let tool_def =
        tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or_else(|| DecodeError::UnknownTool {
                name: name.to_string(),
            })?;

    let Some(remainder) = trimmed.get(name_end..) else {
        return Err(DecodeError::MalformedCall {
            reason: "invalid remainder".to_string(),
        });
    };
    let remainder = remainder.trim_start();
    if remainder.is_empty() {
        return Err(DecodeError::MalformedCall {
            reason: "missing argument object (use {} for none)".to_string(),
        });
    }
    let args_text = if let Some(stripped) = remainder.strip_prefix('{') {
        let mut depth = 1usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end_idx = None;
        let bytes: Vec<char> = stripped.chars().collect();
        for (idx, ch) in bytes.iter().enumerate() {
            if in_string {
                if escaped {
                    escaped = false;
                    continue;
                }
                if *ch == '\\' {
                    escaped = true;
                    continue;
                }
                if *ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match *ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = Some(idx + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end_index) = end_idx else {
            return Err(DecodeError::MalformedCall {
                reason: "unterminated argument object".to_string(),
            });
        };
        let Some(obj) = stripped.get(..end_index) else {
            return Err(DecodeError::MalformedCall {
                reason: "invalid argument object".to_string(),
            });
        };
        let tail = stripped.get(end_index..).unwrap_or("").trim_start();
        if !tail.starts_with(">>") {
            return Err(DecodeError::MalformedCall {
                reason: "missing closing >>".to_string(),
            });
        }
        format!("{{{obj}")
    } else {
        return Err(DecodeError::MalformedCall {
            reason: "missing argument object".to_string(),
        });
    };

    let parsed: Value =
        serde_json::from_str(&args_text).map_err(|err| DecodeError::InvalidArguments {
            tool: tool_def.name.clone(),
            path: "/".to_string(),
            reason: err.to_string(),
        })?;
    let object = parsed
        .as_object()
        .ok_or_else(|| DecodeError::InvalidArguments {
            tool: tool_def.name.clone(),
            path: "/".to_string(),
            reason: "arguments must be a JSON object".to_string(),
        })?;
    validate_call(tool_def, object)?;

    Ok(ToolCall {
        name: tool_def.name.clone(),
        arguments: object.clone(),
    })
}
