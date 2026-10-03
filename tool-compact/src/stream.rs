use serde_json::Value;
use crate::decoder::decode_call;
use crate::error::ToolCompactError;
use crate::schema::ToolRegistry;
use crate::validator::validate_call;

/// Events emitted during stream decoding of compact tool calls.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Emitted as soon as the tool identifier and opening '(' are identified.
    ToolStarted { name: String },

    /// Emitted when a tool call is fully parsed and validated.
    CallComplete { name: String, args: Value },

    /// Emitted immediately upon encountering a syntax error or schema violation.
    Error(ToolCompactError),
}

/// State machine for parsing streaming tokens of compact tool calls.
pub struct CompactStreamDecoder {
    buffer: String,
    registry: Option<ToolRegistry>,
    tool_started_emitted: bool,
    active_tool_name: Option<String>,
    is_failed: bool,
}

impl CompactStreamDecoder {
    /// Creates a new `CompactStreamDecoder` without registry validation.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            registry: None,
            tool_started_emitted: false,
            active_tool_name: None,
            is_failed: false,
        }
    }

    /// Creates a new `CompactStreamDecoder` configured with a `ToolRegistry` for fail-closed validation.
    pub fn with_registry(registry: ToolRegistry) -> Self {
        Self {
            buffer: String::new(),
            registry: Some(registry),
            tool_started_emitted: false,
            active_tool_name: None,
            is_failed: false,
        }
    }

    /// Feeds a chunk of text into the decoder state machine and returns any generated `StreamEvent`s.
    pub fn feed_chunk(&mut self, chunk: &str) -> Vec<StreamEvent> {
        if self.is_failed {
            return vec![StreamEvent::Error(ToolCompactError::Malformed(
                "Stream is in failed state".into(),
            ))];
        }

        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // Check if we have identified the tool name up to '('
        if !self.tool_started_emitted {
            if let Some(open_paren_idx) = self.buffer.find('(') {
                let candidate_name = self.buffer[..open_paren_idx].trim().to_string();
                if candidate_name.is_empty()
                    || !candidate_name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                {
                    self.is_failed = true;
                    events.push(StreamEvent::Error(ToolCompactError::Malformed(format!(
                        "Invalid tool identifier: '{}'",
                        candidate_name
                    ))));
                    return events;
                }

                // Check fail-closed unknown tool rule if registry is present
                if let Some(ref reg) = self.registry {
                    if !reg.contains(&candidate_name) {
                        self.is_failed = true;
                        events.push(StreamEvent::Error(ToolCompactError::UnknownTool(
                            candidate_name,
                        )));
                        return events;
                    }
                }

                self.tool_started_emitted = true;
                self.active_tool_name = Some(candidate_name.clone());
                events.push(StreamEvent::ToolStarted {
                    name: candidate_name,
                });
            }
        }

        // Check if full call ')' is available
        if self.tool_started_emitted {
            // Attempt decoding if closing parenthesis is present or full payload ready
            if self.buffer.trim_end().ends_with(')') {
                let trimmed = self.buffer.trim().to_string();
                match decode_call(&trimmed) {
                    Ok((name, raw_args)) => {
                        // Validate arguments if registry is present
                        if let Some(ref reg) = self.registry {
                            match validate_call(&name, &raw_args, reg) {
                                Ok(validated_args) => {
                                    events.push(StreamEvent::CallComplete {
                                        name,
                                        args: validated_args,
                                    });
                                }
                                Err(err) => {
                                    self.is_failed = true;
                                    events.push(StreamEvent::Error(err));
                                }
                            }
                        } else {
                            events.push(StreamEvent::CallComplete {
                                name,
                                args: raw_args,
                            });
                        }
                    }
                    Err(err) => {
                        self.is_failed = true;
                        events.push(StreamEvent::Error(err));
                    }
                }
            }
        }

        events
    }

    /// Resets the decoder state for processing a new stream.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.tool_started_emitted = false;
        self.active_tool_name = None;
        self.is_failed = false;
    }
}

impl Default for CompactStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}
