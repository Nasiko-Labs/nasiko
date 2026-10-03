use crate::{
    CompactError, MAX_BYTES, MAX_CALLS, Result, ToolCall, ToolDef, check_tools,
    strict_json::StrictValue, validate_call,
};

/// Chunk-fed parser. Calls are staged as complete JSON values arrive, but are
/// released only by finish(): a later invalid call rejects the whole message.
/// String chunks must end at UTF-8 boundaries (as HTTP text deltas do).
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    cursor: usize,
    staged: Vec<ToolCall>,
    error: Option<CompactError>,
    finished: bool,
}
impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        check_tools(tools)?;
        Ok(Self {
            tools: tools.to_vec(),
            buffer: String::new(),
            cursor: 0,
            staged: Vec::new(),
            error: None,
            finished: false,
        })
    }
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        if self.finished {
            return Err(CompactError::Finished);
        }
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if chunk.len() > MAX_BYTES.saturating_sub(self.buffer.len()) {
            self.error = Some(CompactError::ResourceLimit);
            return Err(CompactError::ResourceLimit);
        }
        self.buffer.push_str(chunk);
        let result = self.parse(false);
        if let Err(e) = &result {
            self.error = Some(e.clone());
        }
        result
    }
    pub fn finish(&mut self) -> Result<Vec<ToolCall>> {
        if self.finished {
            return Err(CompactError::Finished);
        }
        self.finished = true;
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        self.parse(true)?;
        Ok(std::mem::take(&mut self.staged))
    }
    fn parse(&mut self, final_chunk: bool) -> Result<()> {
        loop {
            let rest = &self.buffer[self.cursor..];
            let Some(relative) = rest.find("<<") else {
                // Retain at most a marker-prefix suffix for the next chunk.
                let pending = usize::from(rest.ends_with('<'));
                self.cursor = self.buffer.len() - pending;
                return Ok(());
            };
            let start = self.cursor + relative;
            let tail = &self.buffer[start + 2..];
            if tail.is_empty() {
                return if final_chunk {
                    Err(CompactError::MalformedOutput)
                } else {
                    self.cursor = start;
                    Ok(())
                };
            }
            let tail = tail.trim_start();
            let Some(space) = tail.find(char::is_whitespace) else {
                return if final_chunk {
                    Err(CompactError::MalformedOutput)
                } else {
                    self.cursor = start;
                    Ok(())
                };
            };
            let mut name = &tail[..space];
            let mut json = tail[space..].trim_start();
            // Published form: <<call NAME {...}>>. Some models omit the
            // introducer; accept <<NAME {...}>> as an explicit second grammar.
            // A tool literally named "call" is unambiguous when JSON follows it.
            if name == "call" && !json.starts_with('{') {
                let Some(space) = json.find(char::is_whitespace) else {
                    return if final_chunk {
                        Err(CompactError::MalformedOutput)
                    } else {
                        self.cursor = start;
                        Ok(())
                    };
                };
                name = &json[..space];
                json = json[space..].trim_start();
            }
            if !crate::valid_name(name) {
                return Err(CompactError::MalformedOutput);
            }
            let mut values = serde_json::Deserializer::from_str(json).into_iter::<StrictValue>();
            let args = match values.next() {
                Some(Ok(value)) => value.0,
                Some(Err(error)) if error.is_eof() && !final_chunk => {
                    self.cursor = start;
                    return Ok(());
                }
                None if !final_chunk => {
                    self.cursor = start;
                    return Ok(());
                }
                _ => return Err(CompactError::MalformedOutput),
            };
            let suffix = &json[values.byte_offset()..];
            let trimmed = suffix.trim_start();
            if !trimmed.starts_with(">>") {
                if !final_chunk && (trimmed.is_empty() || trimmed == ">") {
                    self.cursor = start;
                    return Ok(());
                }
                return Err(CompactError::MalformedOutput);
            }
            let call = ToolCall {
                name: name.to_owned(),
                arguments: args,
            };
            validate_call(&call, &self.tools)?;
            if self.staged.len() >= MAX_CALLS {
                return Err(CompactError::ResourceLimit);
            }
            self.staged.push(call);
            self.cursor = self.buffer.len() - trimmed.len() + 2;
        }
    }
}
