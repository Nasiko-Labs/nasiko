use super::parser::parse_object;
use crate::{
    CompactError, Result, ToolCall, ToolDef, analyze_tools,
    schema::{self, CanonicalTool},
};
use std::collections::BTreeMap;

const MARKER: &[u8] = b"<<call";
const MAX_CALL_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_CALLS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Searching,
    AfterMarker,
    ToolName,
    JsonStart,
    Json,
    CloseStart,
    CloseEnd,
}

/// Incremental, string-aware decoder with sticky failures and atomic finalization.
///
/// `push` results are provisional. Stage them; execute only after `finish`
/// succeeds. `finish` returns the complete validated response and consumes self.
pub struct StreamDecoder {
    tools: BTreeMap<String, CanonicalTool>,
    state: State,
    marker_progress: usize,
    name: String,
    json: String,
    nesting: Vec<char>,
    in_string: bool,
    escape_next: bool,
    call_bytes: usize,
    response_bytes: usize,
    calls: Vec<ToolCall>,
    failure: Option<CompactError>,
}

impl StreamDecoder {
    /// Preflight the complete tool set; unsupported schemas cannot be decoded unsafely.
    pub fn new(tools: &[ToolDef]) -> Result<Self> {
        let tools = analyze_tools(tools)?
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
        Ok(Self {
            tools,
            state: State::Searching,
            marker_progress: 0,
            name: String::new(),
            json: String::new(),
            nesting: Vec::new(),
            in_string: false,
            escape_next: false,
            call_bytes: 0,
            response_bytes: 0,
            calls: Vec::new(),
            failure: None,
        })
    }

    /// Return newly completed validated calls, pending whole-response finalization.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        self.response_bytes = self.response_bytes.saturating_add(chunk.len());
        let result = if self.response_bytes > MAX_RESPONSE_BYTES {
            Err(CompactError::LimitExceeded("response bytes"))
        } else {
            self.consume(chunk)
        };
        if let Err(error) = &result {
            self.calls.clear();
            self.failure = Some(error.clone());
        }
        result
    }

    /// Commit the whole validated response, or reject all provisional calls.
    pub fn finish(self) -> Result<Vec<ToolCall>> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.state != State::Searching || self.marker_progress != 0 {
            return Err(CompactError::IncompleteCall);
        }
        Ok(self.calls)
    }

    fn consume(&mut self, chunk: &str) -> Result<Vec<ToolCall>> {
        let mut completed = Vec::new();
        for character in chunk.chars() {
            if self.state != State::Searching {
                self.call_bytes = self.call_bytes.saturating_add(character.len_utf8());
                if self.call_bytes > MAX_CALL_BYTES {
                    return Err(CompactError::LimitExceeded("call bytes"));
                }
            }
            match self.state {
                State::Searching => self.search(character),
                State::AfterMarker => {
                    if !character.is_whitespace() {
                        return Err(CompactError::MalformedCall);
                    }
                    self.state = State::ToolName;
                }
                State::ToolName => self.read_name(character)?,
                State::JsonStart => {
                    if character.is_whitespace() {
                        continue;
                    }
                    self.start_json(character)?;
                }
                State::Json => self.read_json(character)?,
                State::CloseStart => {
                    if character.is_whitespace() {
                        continue;
                    }
                    if character != '>' {
                        return Err(CompactError::MalformedCall);
                    }
                    self.state = State::CloseEnd;
                }
                State::CloseEnd => {
                    if character != '>' {
                        return Err(CompactError::MalformedCall);
                    }
                    let call = self.complete()?;
                    self.calls.push(call.clone());
                    completed.push(call);
                }
            }
        }
        Ok(completed)
    }

    fn search(&mut self, character: char) {
        if MARKER
            .get(self.marker_progress)
            .is_some_and(|&byte| character == char::from(byte))
        {
            self.marker_progress += 1;
            if self.marker_progress == MARKER.len() {
                self.state = State::AfterMarker;
                self.marker_progress = 0;
                self.call_bytes = MARKER.len();
            }
        } else {
            // The only proper self-overlap of the marker is its initial '<'.
            self.marker_progress = if character == '<' {
                if self.marker_progress >= 2 { 2 } else { 1 }
            } else {
                0
            };
        }
    }

    fn read_name(&mut self, character: char) -> Result<()> {
        if character.is_whitespace() {
            if self.name.is_empty() {
                return Ok(());
            }
            if !schema::safe_identifier(&self.name) {
                return Err(CompactError::MalformedCall);
            }
            if !self.tools.contains_key(&self.name) {
                return Err(CompactError::UnknownTool(self.name.clone()));
            }
            self.state = State::JsonStart;
        } else {
            if !character.is_ascii_alphanumeric() && !matches!(character, '_' | '.' | ':' | '-') {
                return Err(CompactError::MalformedCall);
            }
            self.name.push(character);
        }
        Ok(())
    }

    fn start_json(&mut self, character: char) -> Result<()> {
        if character != '{' {
            return Err(CompactError::MalformedCall);
        }
        self.json.push(character);
        self.nesting.push(character);
        self.state = State::Json;
        Ok(())
    }

    fn read_json(&mut self, character: char) -> Result<()> {
        self.json.push(character);
        if self.in_string {
            if self.escape_next {
                self.escape_next = false;
            } else if character == '\\' {
                self.escape_next = true;
            } else if character == '"' {
                self.in_string = false;
            }
        } else {
            match character {
                '"' => self.in_string = true,
                '{' | '[' => {
                    if self.nesting.len() >= schema::MAX_DEPTH {
                        return Err(CompactError::LimitExceeded("JSON depth"));
                    }
                    self.nesting.push(character);
                }
                '}' | ']' => {
                    let expected = if character == '}' { '{' } else { '[' };
                    if self.nesting.pop() != Some(expected) {
                        return Err(CompactError::MalformedCall);
                    }
                    if self.nesting.is_empty() {
                        self.state = State::CloseStart;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<ToolCall> {
        if self.calls.len() >= MAX_CALLS {
            return Err(CompactError::LimitExceeded("call count"));
        }
        let arguments = parse_object(&self.json)?;
        let tool = self
            .tools
            .get(&self.name)
            .ok_or_else(|| CompactError::UnknownTool(self.name.clone()))?;
        schema::validate(tool, &arguments)?;
        let call = ToolCall {
            name: std::mem::take(&mut self.name),
            arguments,
        };
        self.json.clear();
        self.nesting.clear();
        self.in_string = false;
        self.escape_next = false;
        self.state = State::Searching;
        Ok(call)
    }
}
