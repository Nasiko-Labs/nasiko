use std::collections::HashMap;
use crate::error::CompactError;
use crate::limits::Limits;
use crate::strict_json::StrictJsonParser;
use crate::types::{DecodedResponse, ToolCall, ToolDef};
use crate::validate::validate_arguments;

const OPENER: &[u8] = b"<<call";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Object,
    Array,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    MarkerPrefix,
    NameWhitespace,
    Name,
    BeforeJson,
    Json,
    TerminatorWhitespace,
    TerminatorSecond,
    Finished,
    Failed,
}

pub struct StreamDecoder<'a> {
    tools: &'a [ToolDef],
    tool_map: HashMap<&'a str, &'a ToolDef>,
    limits: Limits,
    state: State,
    total_bytes: usize,
    text_output: Vec<u8>,
    pending_prefix: Vec<u8>,
    current_tool_name: Vec<u8>,
    current_json_bytes: Vec<u8>,
    container_stack: Vec<Container>,
    in_string: bool,
    escape: bool,
    pending_calls: Vec<ToolCall>,
    error: Option<CompactError>,
}

impl<'a> StreamDecoder<'a> {
    pub fn new(tools: &'a [ToolDef], limits: Limits) -> Result<Self, CompactError> {
        if tools.len() > limits.max_tools {
            return Err(CompactError::LimitExceeded {
                limit: "max_tools",
                value: tools.len(),
                max: limits.max_tools,
            });
        }

        let mut tool_map = HashMap::new();
        for t in tools {
            tool_map.insert(t.name.as_str(), t);
        }

        Ok(Self {
            tools,
            tool_map,
            limits,
            state: State::Text,
            total_bytes: 0,
            text_output: Vec::new(),
            pending_prefix: Vec::new(),
            current_tool_name: Vec::new(),
            current_json_bytes: Vec::new(),
            container_stack: Vec::new(),
            in_string: false,
            escape: false,
            pending_calls: Vec::new(),
            error: None,
        })
    }

    pub fn tools(&self) -> &'a [ToolDef] {
        self.tools
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), CompactError> {
        if self.state == State::Failed {
            return Err(self.error.clone().unwrap_or(CompactError::MalformedCall {
                reason: "decoder is in failed state".to_string(),
                offset: self.total_bytes,
            }));
        }

        for &b in chunk {
            self.total_bytes += 1;
            if self.total_bytes > self.limits.max_response_bytes {
                let err = CompactError::LimitExceeded {
                    limit: "max_response_bytes",
                    value: self.total_bytes,
                    max: self.limits.max_response_bytes,
                };
                self.fail(err.clone());
                return Err(err);
            }

            if let Err(e) = self.process_byte(b) {
                self.fail(e.clone());
                return Err(e);
            }
        }

        Ok(())
    }

    fn fail(&mut self, err: CompactError) {
        self.state = State::Failed;
        self.error = Some(err);
        self.pending_calls.clear();
    }

    fn process_byte(&mut self, b: u8) -> Result<(), CompactError> {
        match self.state {
            State::Text => {
                if b == b'<' {
                    self.pending_prefix.clear();
                    self.pending_prefix.push(b);
                    self.state = State::MarkerPrefix;
                } else {
                    self.text_output.push(b);
                }
            }
            State::MarkerPrefix => {
                self.pending_prefix.push(b);
                let matched_len = self.pending_prefix.len();
                if matched_len <= OPENER.len() && OPENER.starts_with(&self.pending_prefix) {
                    if matched_len == OPENER.len() {
                        // Full <<call matched!
                        self.pending_prefix.clear();
                        self.state = State::NameWhitespace;
                    }
                } else {
                    // Mismatch: flush pending prefix text using overlap-aware scan
                    let flushed = std::mem::take(&mut self.pending_prefix);
                    // Check if there is an overlapping '<' inside flushed
                    let mut i = 0;
                    while i < flushed.len() {
                        if flushed[i] == b'<' {
                            let candidate = &flushed[i..];
                            if OPENER.starts_with(candidate) {
                                self.pending_prefix.extend_from_slice(candidate);
                                self.state = State::MarkerPrefix;
                                break;
                            }
                        }
                        self.text_output.push(flushed[i]);
                        i += 1;
                    }
                    if self.pending_prefix.is_empty() {
                        self.state = State::Text;
                    }
                }
            }
            State::NameWhitespace => {
                if b.is_ascii_whitespace() {
                    // Required at least one whitespace byte after <<call
                    self.current_tool_name.clear();
                    self.state = State::Name;
                } else {
                    return Err(CompactError::MalformedCall {
                        reason: "expected whitespace after <<call".to_string(),
                        offset: self.total_bytes - 1,
                    });
                }
            }
            State::Name => {
                if b.is_ascii_whitespace() {
                    if self.current_tool_name.is_empty() {
                        return Err(CompactError::MalformedCall {
                            reason: "tool name cannot be empty".to_string(),
                            offset: self.total_bytes - 1,
                        });
                    }
                    self.state = State::BeforeJson;
                } else if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                    self.current_tool_name.push(b);
                    if self.current_tool_name.len() > self.limits.max_tool_name_bytes {
                        return Err(CompactError::LimitExceeded {
                            limit: "max_tool_name_bytes",
                            value: self.current_tool_name.len(),
                            max: self.limits.max_tool_name_bytes,
                        });
                    }
                } else {
                    return Err(CompactError::MalformedCall {
                        reason: format!("invalid character '{}' in tool name", b as char),
                        offset: self.total_bytes - 1,
                    });
                }
            }
            State::BeforeJson => {
                if b.is_ascii_whitespace() {
                    // skip whitespace before '{'
                } else if b == b'{' {
                    self.current_json_bytes.clear();
                    self.current_json_bytes.push(b);
                    self.container_stack.clear();
                    self.container_stack.push(Container::Object);
                    self.in_string = false;
                    self.escape = false;
                    self.state = State::Json;
                } else {
                    return Err(CompactError::MalformedCall {
                        reason: format!("expected '{{' to begin arguments, found '{}'", b as char),
                        offset: self.total_bytes - 1,
                    });
                }
            }
            State::Json => {
                self.current_json_bytes.push(b);
                if self.current_json_bytes.len() > self.limits.max_argument_bytes {
                    return Err(CompactError::LimitExceeded {
                        limit: "max_argument_bytes",
                        value: self.current_json_bytes.len(),
                        max: self.limits.max_argument_bytes,
                    });
                }

                if self.in_string {
                    if self.escape {
                        self.escape = false;
                    } else if b == b'\\' {
                        self.escape = true;
                    } else if b == b'"' {
                        self.in_string = false;
                    }
                } else {
                    match b {
                        b'"' => self.in_string = true,
                        b'{' => {
                            self.container_stack.push(Container::Object);
                            if self.container_stack.len() > self.limits.max_json_depth {
                                return Err(CompactError::LimitExceeded {
                                    limit: "max_json_depth",
                                    value: self.container_stack.len(),
                                    max: self.limits.max_json_depth,
                                });
                            }
                        }
                        b'[' => {
                            self.container_stack.push(Container::Array);
                            if self.container_stack.len() > self.limits.max_json_depth {
                                return Err(CompactError::LimitExceeded {
                                    limit: "max_json_depth",
                                    value: self.container_stack.len(),
                                    max: self.limits.max_json_depth,
                                });
                            }
                        }
                        b'}' => {
                            match self.container_stack.pop() {
                                Some(Container::Object) => {
                                    if self.container_stack.is_empty() {
                                        // Root object closed!
                                        self.state = State::TerminatorWhitespace;
                                    }
                                }
                                _ => {
                                    return Err(CompactError::InvalidJson {
                                        reason: "mismatched closing delimiter '}'".to_string(),
                                        offset: self.total_bytes - 1,
                                    });
                                }
                            }
                        }
                        b']' => {
                            match self.container_stack.pop() {
                                Some(Container::Array) => {}
                                _ => {
                                    return Err(CompactError::InvalidJson {
                                        reason: "mismatched closing delimiter ']'".to_string(),
                                        offset: self.total_bytes - 1,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            State::TerminatorWhitespace => {
                if b.is_ascii_whitespace() {
                    // skip whitespace before '>'
                } else if b == b'>' {
                    self.state = State::TerminatorSecond;
                } else {
                    return Err(CompactError::MalformedCall {
                        reason: format!("expected '>' to terminate call, found '{}'", b as char),
                        offset: self.total_bytes - 1,
                    });
                }
            }
            State::TerminatorSecond => {
                if b == b'>' {
                    // Call is fully framed! Validate and append
                    self.finalize_call()?;
                    self.state = State::Text;
                } else {
                    return Err(CompactError::MalformedCall {
                        reason: format!("expected second '>' to terminate call, found '{}'", b as char),
                        offset: self.total_bytes - 1,
                    });
                }
            }
            State::Finished | State::Failed => {}
        }
        Ok(())
    }

    fn finalize_call(&mut self) -> Result<(), CompactError> {
        if self.pending_calls.len() >= self.limits.max_calls {
            return Err(CompactError::LimitExceeded {
                limit: "max_calls",
                value: self.pending_calls.len() + 1,
                max: self.limits.max_calls,
            });
        }

        let json_str = std::str::from_utf8(&self.current_json_bytes).map_err(|e| CompactError::InvalidUtf8 {
            reason: e.to_string(),
            offset: self.total_bytes - self.current_json_bytes.len(),
        })?;

        let tool_name = std::str::from_utf8(&self.current_tool_name).map_err(|e| CompactError::InvalidUtf8 {
            reason: e.to_string(),
            offset: self.total_bytes - self.current_tool_name.len(),
        })?.to_string();

        // Strict JSON parsing with duplicate key rejection
        let mut parser = StrictJsonParser::new(json_str, &self.limits);
        let (parsed_val, _) = parser.parse_object()?;

        // Tool name lookup
        let tool_def = self.tool_map.get(tool_name.as_str()).ok_or_else(|| CompactError::UnknownTool {
            name: tool_name.clone(),
            offset: self.total_bytes - self.current_json_bytes.len() - tool_name.len(),
        })?;

        // Validate against original schema
        validate_arguments(&tool_name, &parsed_val, &tool_def.parameters, false)?;

        self.pending_calls.push(ToolCall {
            name: tool_name,
            arguments: parsed_val,
            arguments_json: json_str.to_string(),
        });

        self.current_tool_name.clear();
        self.current_json_bytes.clear();
        Ok(())
    }

    pub fn finish(mut self) -> Result<DecodedResponse, CompactError> {
        if self.state == State::Failed {
            return Err(self.error.unwrap_or(CompactError::MalformedCall {
                reason: "failed state".to_string(),
                offset: self.total_bytes,
            }));
        }

        match self.state {
            State::Text | State::MarkerPrefix => {
                // An unmatched '<' remains ordinary text.
                // A trailing partial opener with 2 or more bytes (e.g. '<<', '<<ca') is an incomplete-call error.
                if self.pending_prefix == b"<" {
                    self.text_output.push(b'<');
                    self.pending_prefix.clear();
                } else if !self.pending_prefix.is_empty() {
                    return Err(CompactError::IncompleteCall {
                        reason: "incomplete <<call marker at end of response".to_string(),
                        offset: self.total_bytes - self.pending_prefix.len(),
                    });
                }

                let text = std::str::from_utf8(&self.text_output).map_err(|e| CompactError::InvalidUtf8 {
                    reason: e.to_string(),
                    offset: self.total_bytes,
                })?.to_string();

                self.state = State::Finished;
                Ok(DecodedResponse {
                    text,
                    calls: self.pending_calls,
                })
            }
            State::NameWhitespace | State::Name | State::BeforeJson | State::Json | State::TerminatorWhitespace | State::TerminatorSecond => {
                Err(CompactError::IncompleteCall {
                    reason: "response ended inside incomplete call".to_string(),
                    offset: self.total_bytes,
                })
            }
            State::Finished => {
                Err(CompactError::MalformedCall {
                    reason: "decoder already finished".to_string(),
                    offset: self.total_bytes,
                })
            }
            State::Failed => unreachable!(),
        }
    }
}

pub fn decode_response_with_limits(
    response: &str,
    tools: &[ToolDef],
    limits: Limits,
) -> Result<DecodedResponse, CompactError> {
    let mut decoder = StreamDecoder::new(tools, limits)?;
    decoder.push(response.as_bytes())?;
    decoder.finish()
}

pub fn decode_response(response: &str, tools: &[ToolDef]) -> Result<DecodedResponse, CompactError> {
    decode_response_with_limits(response, tools, Limits::default())
}

pub fn decode_calls_with_limits(
    response: &str,
    tools: &[ToolDef],
    limits: Limits,
) -> Result<Vec<ToolCall>, CompactError> {
    let resp = decode_response_with_limits(response, tools, limits)?;
    Ok(resp.calls)
}

pub fn decode_calls(response: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    decode_calls_with_limits(response, tools, Limits::default())
}
