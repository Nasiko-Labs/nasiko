use crate::schema::Catalog;
use crate::{CompactError, DecodedOutput, Limits, Result, ToolCall, ToolDef, strict_json};

const MARKER: &[u8] = b"<<call ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    Name,
    ArgumentsStart,
    Arguments,
    Closing,
}

/// Linear incremental parser. Each byte is scanned once, even with one-byte chunks.
/// Calls are buffered until `finish`; any failure poisons the entire decoder.
pub struct StreamDecoder {
    catalog: Catalog,
    limits: Limits,
    state: State,
    total_bytes: usize,
    matched: usize,
    closing: usize,
    name: Vec<u8>,
    arguments: Vec<u8>,
    stack: Vec<u8>,
    in_string: bool,
    escaped: bool,
    text: Vec<u8>,
    calls: Vec<ToolCall>,
    failure: Option<CompactError>,
}

impl StreamDecoder {
    /// Prepare validation once for this response.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        Self::with_limits(tools, Limits::default())
    }

    /// Construct with explicit resource bounds supplied by the embedding application.
    pub fn with_limits(tools: &[ToolDef], limits: Limits) -> Result<Self> {
        Ok(Self {
            catalog: Catalog::new(tools)?,
            limits,
            state: State::Text,
            total_bytes: 0,
            matched: 0,
            closing: 0,
            name: Vec::new(),
            arguments: Vec::new(),
            stack: Vec::new(),
            in_string: false,
            escaped: false,
            text: Vec::new(),
            calls: Vec::new(),
            failure: None,
        })
    }

    /// Feed a provider text delta. Chunk boundaries have no semantic meaning.
    pub fn push(&mut self, chunk: &str) -> Result<()> {
        self.push_bytes(chunk.as_bytes())
    }

    /// Feed raw bytes, including chunks split within a UTF-8 code point.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.total_bytes = self.total_bytes.saturating_add(chunk.len());
        let result = if self.total_bytes > self.limits.max_output_bytes {
            Err(CompactError::LimitExceeded)
        } else {
            chunk.iter().try_for_each(|byte| self.consume(*byte))
        };
        if let Err(error) = &result {
            self.failure = Some(error.clone());
            self.calls.clear();
        }
        result
    }

    /// Validate end-of-stream and release the complete batch atomically.
    pub fn finish(self) -> Result<DecodedOutput> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.state != State::Text || self.matched >= 2 {
            return Err(CompactError::IncompleteCall);
        }
        let mut text = self.text;
        text.extend_from_slice(&MARKER[..self.matched]);
        let text = String::from_utf8(text).map_err(|_| CompactError::MalformedOutput)?;
        Ok(DecodedOutput {
            text,
            calls: self.calls,
        })
    }

    fn consume(&mut self, byte: u8) -> Result<()> {
        match self.state {
            State::Text => self.consume_text(byte),
            State::Name => self.consume_name(byte),
            State::ArgumentsStart => {
                if byte.is_ascii_whitespace() {
                    return Ok(());
                }
                if byte != b'{' {
                    return Err(CompactError::InvalidArguments);
                }
                self.state = State::Arguments;
                self.consume_arguments(byte)
            }
            State::Arguments => self.consume_arguments(byte),
            State::Closing => self.consume_closing(byte),
        }
    }

    fn consume_text(&mut self, byte: u8) -> Result<()> {
        if byte == MARKER[self.matched] {
            self.matched += 1;
            if self.matched == MARKER.len() {
                self.matched = 0;
                self.name.clear();
                self.arguments.clear();
                self.state = State::Name;
            }
            return Ok(());
        }
        if self.matched >= 2 {
            // A reserved call-marker prefix must not turn into a plain answer on a typo.
            return Err(CompactError::MalformedOutput);
        }
        self.text.extend_from_slice(&MARKER[..self.matched]);
        self.matched = 0;
        if byte == MARKER[0] {
            self.matched = 1;
        } else {
            self.text.push(byte);
        }
        Ok(())
    }

    fn consume_name(&mut self, byte: u8) -> Result<()> {
        if byte.is_ascii_whitespace() {
            if self.name.is_empty() {
                return Err(CompactError::MalformedOutput);
            }
            self.state = State::ArgumentsStart;
        } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
            self.name.push(byte);
            if self.name.len() > 64 {
                return Err(CompactError::LimitExceeded);
            }
        } else {
            return Err(CompactError::MalformedOutput);
        }
        Ok(())
    }

    fn consume_arguments(&mut self, byte: u8) -> Result<()> {
        if self.arguments.len() >= self.limits.max_call_bytes {
            return Err(CompactError::LimitExceeded);
        }
        self.arguments.push(byte);
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.in_string = false;
            }
            return Ok(());
        }
        match byte {
            b'"' => self.in_string = true,
            b'{' | b'[' => {
                if self.stack.len() >= self.limits.max_json_depth {
                    return Err(CompactError::LimitExceeded);
                }
                self.stack.push(byte);
            }
            b'}' | b']' => {
                let expected = if byte == b'}' { b'{' } else { b'[' };
                if self.stack.pop() != Some(expected) {
                    return Err(CompactError::InvalidArguments);
                }
                if self.stack.is_empty() {
                    self.state = State::Closing;
                    self.closing = 0;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn consume_closing(&mut self, byte: u8) -> Result<()> {
        if self.closing == 0 && byte.is_ascii_whitespace() {
            return Ok(());
        }
        if byte != b'>' {
            return Err(CompactError::MalformedOutput);
        }
        self.closing += 1;
        if self.closing == 2 {
            if self.calls.len() >= self.limits.max_calls {
                return Err(CompactError::LimitExceeded);
            }
            let name =
                String::from_utf8(self.name.clone()).map_err(|_| CompactError::MalformedOutput)?;
            let json =
                std::str::from_utf8(&self.arguments).map_err(|_| CompactError::InvalidArguments)?;
            let arguments = strict_json::parse(json).map_err(|_| CompactError::InvalidArguments)?;
            let call = ToolCall { name, arguments };
            self.catalog.validate(&call)?;
            self.calls.push(call);
            self.state = State::Text;
        }
        Ok(())
    }
}

/// Decode text using exactly the same state machine as streamed output.
pub fn decode_output(text: &str, tools: &[ToolDef]) -> Result<DecodedOutput> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish()
}

/// Decode only the tool calls, preserving argument values and their order.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>> {
    Ok(decode_output(text, tools)?.calls)
}

/// Render supplied calls for offline round-trip evaluation. This performs no inference.
pub fn render_calls(calls: &[ToolCall], tools: &[ToolDef]) -> Result<String> {
    let catalog = Catalog::new(tools)?;
    let mut text = String::new();
    for call in calls {
        catalog.validate(call)?;
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("<<call ");
        text.push_str(&call.name);
        text.push(' ');
        text.push_str(&call.arguments.to_string());
        text.push_str(">>");
    }
    Ok(text)
}
