//! Chunk-agnostic streaming decoder for compact tool calling.

use serde_json::Value;

use crate::error::CompactToolError;
use crate::types::{StreamEvent, ToolCall, ToolDefinition};
use crate::validator::validate_tool_call;

/// Incremental streaming parser that decodes text deltas and tool call events from incoming chunks.
#[derive(Debug, Clone)]
pub struct StreamDecoder {
    tools: Vec<ToolDefinition>,
    state: StreamState,
    text_buffer: String,
    completed_calls: Vec<ToolCall>,
    call_index: usize,
}

impl StreamDecoder {
    /// Creates a new `StreamDecoder` initialized with the given registered tools.
    pub fn new(tools: Vec<ToolDefinition>) -> Self {
        Self {
            tools,
            state: StreamState::Text,
            text_buffer: String::new(),
            completed_calls: Vec::new(),
            call_index: 0,
        }
    }

    /// Feeds an incoming text chunk into the decoder, yielding any newly parsed events.
    pub fn feed(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, CompactToolError> {
        let mut events = Vec::new();

        for ch in chunk.chars() {
            match &mut self.state {
                StreamState::Text => {
                    if ch == '<' {
                        self.state = StreamState::PotentialOpen1;
                    } else {
                        self.text_buffer.push(ch);
                        events.push(StreamEvent::TextDelta(ch.to_string()));
                    }
                }
                StreamState::PotentialOpen1 => {
                    if ch == '<' {
                        self.state = StreamState::CallKeyword { matched: 0 };
                    } else {
                        self.text_buffer.push('<');
                        self.text_buffer.push(ch);
                        events.push(StreamEvent::TextDelta(format!("<{}", ch)));
                        self.state = StreamState::Text;
                    }
                }
                StreamState::CallKeyword { matched } => {
                    let target = ['c', 'a', 'l', 'l'];
                    if *matched < target.len() {
                        if ch == target[*matched] {
                            *matched += 1;
                        } else {
                            // Mismatch during "call" keyword
                            let mut flush = String::from("<<");
                            for &c in &target[..*matched] {
                                flush.push(c);
                            }
                            flush.push(ch);
                            self.text_buffer.push_str(&flush);
                            events.push(StreamEvent::TextDelta(flush));
                            self.state = StreamState::Text;
                        }
                    } else {
                        // All 4 characters 'c','a','l','l' matched. Next must be whitespace.
                        if ch.is_whitespace() {
                            self.state = StreamState::ToolNamePre;
                        } else {
                            let flush = format!("<<call{}", ch);
                            self.text_buffer.push_str(&flush);
                            events.push(StreamEvent::TextDelta(flush));
                            self.state = StreamState::Text;
                        }
                    }
                }
                StreamState::ToolNamePre => {
                    if ch.is_whitespace() {
                        // Consume whitespace between 'call' and tool name
                    } else if is_ident_start(ch) {
                        let mut name = String::new();
                        name.push(ch);
                        self.state = StreamState::ToolName { name };
                    } else {
                        let flush = format!("<<call {}", ch);
                        self.text_buffer.push_str(&flush);
                        events.push(StreamEvent::TextDelta(flush));
                        self.state = StreamState::Text;
                    }
                }
                StreamState::ToolName { name } => {
                    if is_ident_continue(ch) {
                        name.push(ch);
                    } else if ch.is_whitespace() {
                        let tool_name = name.clone();
                        self.state = StreamState::ToolArgsPre { tool_name };
                    } else if ch == '{' {
                        let tool_name = name.clone();
                        let call_id = format!("call_{}_{}", self.call_index, tool_name);
                        events.push(StreamEvent::ToolCallStart {
                            index: self.call_index,
                            id: call_id.clone(),
                            name: tool_name.clone(),
                        });
                        events.push(StreamEvent::ToolCallArgsDelta {
                            index: self.call_index,
                            delta: "{".to_string(),
                        });
                        self.state = StreamState::JsonScan {
                            tool_name,
                            call_id,
                            args_buf: "{".to_string(),
                            depth: 1,
                            in_string: false,
                            escaped: false,
                        };
                    } else {
                        let flush = format!("<<call {}{}", name, ch);
                        self.text_buffer.push_str(&flush);
                        events.push(StreamEvent::TextDelta(flush));
                        self.state = StreamState::Text;
                    }
                }
                StreamState::ToolArgsPre { tool_name } => {
                    if ch.is_whitespace() {
                        // Consume whitespace before '{'
                    } else if ch == '{' {
                        let tool_name = tool_name.clone();
                        let call_id = format!("call_{}_{}", self.call_index, tool_name);
                        events.push(StreamEvent::ToolCallStart {
                            index: self.call_index,
                            id: call_id.clone(),
                            name: tool_name.clone(),
                        });
                        events.push(StreamEvent::ToolCallArgsDelta {
                            index: self.call_index,
                            delta: "{".to_string(),
                        });
                        self.state = StreamState::JsonScan {
                            tool_name,
                            call_id,
                            args_buf: "{".to_string(),
                            depth: 1,
                            in_string: false,
                            escaped: false,
                        };
                    } else {
                        let flush = format!("<<call {} {}", tool_name, ch);
                        self.text_buffer.push_str(&flush);
                        events.push(StreamEvent::TextDelta(flush));
                        self.state = StreamState::Text;
                    }
                }
                StreamState::JsonScan {
                    tool_name,
                    call_id,
                    args_buf,
                    depth,
                    in_string,
                    escaped,
                } => {
                    if *in_string {
                        args_buf.push(ch);
                        events.push(StreamEvent::ToolCallArgsDelta {
                            index: self.call_index,
                            delta: ch.to_string(),
                        });
                        if *escaped {
                            *escaped = false;
                        } else if ch == '\\' {
                            *escaped = true;
                        } else if ch == '"' {
                            *in_string = false;
                        }
                    } else {
                        if ch == '"' {
                            *in_string = true;
                            args_buf.push(ch);
                            events.push(StreamEvent::ToolCallArgsDelta {
                                index: self.call_index,
                                delta: ch.to_string(),
                            });
                        } else if ch == '{' {
                            *depth += 1;
                            args_buf.push(ch);
                            events.push(StreamEvent::ToolCallArgsDelta {
                                index: self.call_index,
                                delta: ch.to_string(),
                            });
                        } else if ch == '}' {
                            *depth = depth.saturating_sub(1);
                            args_buf.push(ch);
                            events.push(StreamEvent::ToolCallArgsDelta {
                                index: self.call_index,
                                delta: ch.to_string(),
                            });
                        } else if *depth == 0 && ch == '>' {
                            self.state = StreamState::PotentialClose1 {
                                tool_name: tool_name.clone(),
                                call_id: call_id.clone(),
                                args_buf: args_buf.clone(),
                            };
                        } else {
                            args_buf.push(ch);
                            events.push(StreamEvent::ToolCallArgsDelta {
                                index: self.call_index,
                                delta: ch.to_string(),
                            });
                        }
                    }
                }
                StreamState::PotentialClose1 {
                    tool_name,
                    call_id,
                    args_buf,
                } => {
                    if ch == '>' {
                        // Tool call closed!
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

                        validate_tool_call(tool_name, &parsed_json, &self.tools)?;

                        let canonical_arguments = serde_json::to_string(&parsed_json)
                            .unwrap_or_else(|_| raw_args.to_string());

                        let call = ToolCall {
                            id: call_id.clone(),
                            name: tool_name.clone(),
                            arguments: canonical_arguments,
                        };

                        self.completed_calls.push(call.clone());
                        events.push(StreamEvent::ToolCallComplete {
                            index: self.call_index,
                            call,
                        });

                        self.call_index += 1;
                        self.state = StreamState::Text;
                    } else {
                        // Not a closing '>>'
                        events.push(StreamEvent::ToolCallArgsDelta {
                            index: self.call_index,
                            delta: format!(">{}", ch),
                        });
                        let mut new_buf = args_buf.clone();
                        new_buf.push('>');
                        new_buf.push(ch);
                        self.state = StreamState::JsonScan {
                            tool_name: tool_name.clone(),
                            call_id: call_id.clone(),
                            args_buf: new_buf,
                            depth: 0,
                            in_string: false,
                            escaped: false,
                        };
                    }
                }
            }
        }

        Ok(events)
    }

    /// Alias for `feed` to match alternative streaming API conventions.
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, CompactToolError> {
        self.feed(chunk)
    }

    /// Flushes any buffered content at the end of the stream and verifies completeness.
    ///
    /// Returns `Err(CompactToolError::UnclosedCallMarker)` if the stream finishes
    /// while inside an open tool call.
    pub fn finish(&mut self) -> Result<Vec<StreamEvent>, CompactToolError> {
        let mut events = Vec::new();
        match &self.state {
            StreamState::Text => Ok(events),
            StreamState::PotentialOpen1 => {
                self.text_buffer.push('<');
                events.push(StreamEvent::TextDelta("<".to_string()));
                self.state = StreamState::Text;
                Ok(events)
            }
            StreamState::CallKeyword { matched } => {
                let mut flush = String::from("<<");
                let target = ['c', 'a', 'l', 'l'];
                for &c in &target[..*matched] {
                    flush.push(c);
                }
                self.text_buffer.push_str(&flush);
                events.push(StreamEvent::TextDelta(flush));
                self.state = StreamState::Text;
                Ok(events)
            }
            StreamState::ToolNamePre
            | StreamState::ToolName { .. }
            | StreamState::ToolArgsPre { .. }
            | StreamState::JsonScan { .. }
            | StreamState::PotentialClose1 { .. } => Err(CompactToolError::UnclosedCallMarker),
        }
    }

    /// Returns a slice of all successfully completed tool calls so far.
    pub fn completed_calls(&self) -> &[ToolCall] {
        &self.completed_calls
    }

    /// Returns the accumulated plain text content parsed so far.
    pub fn current_text(&self) -> &str {
        &self.text_buffer
    }
}

#[derive(Debug, Clone)]
enum StreamState {
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
        call_id: String,
        args_buf: String,
        depth: usize,
        in_string: bool,
        escaped: bool,
    },
    PotentialClose1 {
        tool_name: String,
        call_id: String,
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
