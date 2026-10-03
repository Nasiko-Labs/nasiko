use crate::{
    CompactError, MAX_BYTES, MAX_CALLS, MAX_DEPTH, ToolCall, ToolDef, check_tools, name_char,
    validate_call,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

/// serde_json's Value normally accepts duplicate keys; calls must reject them.
pub(crate) struct UniqueValue(pub Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Number(
                    Number::from_f64(v).ok_or_else(|| E::custom("nonfinite number"))?,
                )))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut a = Vec::new();
                while let Some(v) = seq.next_element::<UniqueValue>()? {
                    a.push(v.0);
                }
                Ok(UniqueValue(Value::Array(a)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut m = Map::new();
                while let Some(k) = map.next_key::<String>()? {
                    if m.contains_key(&k) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    m.insert(k, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(m)))
            }
        }
        d.deserialize_any(UniqueVisitor)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedResponse {
    pub calls: Vec<ToolCall>,
    /// Original text outside encoded call spans, in its original order.
    pub text: String,
}

#[derive(Debug)]
enum State {
    Text,
    Name {
        start: usize,
        prefixed: bool,
    },
    AfterCall,
    BeforeJson,
    Json {
        start: usize,
        stack: Vec<u8>,
        quoted: bool,
        escaped: bool,
    },
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Frame {
    Brackets,
    Native,
    Parenthesized,
}

/// Incrementally scans chunks once. Calls are withheld until finish validates the
/// entire response, so a later bad call cannot release an earlier executable call.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    buffer: String,
    cursor: usize,
    call_start: usize,
    name: String,
    args: Option<Value>,
    state: State,
    frame: Frame,
    calls: Vec<ToolCall>,
    spans: Vec<(usize, usize, bool)>,
    error: Option<CompactError>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Result<Self, CompactError> {
        check_tools(tools)?;
        Ok(Self {
            tools: tools.into(),
            buffer: String::new(),
            cursor: 0,
            call_start: 0,
            name: String::new(),
            args: None,
            state: State::Text,
            frame: Frame::Brackets,
            calls: Vec::new(),
            spans: Vec::new(),
            error: None,
        })
    }
    pub fn push(&mut self, chunk: &str) -> Result<(), CompactError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let result = if self.buffer.len().saturating_add(chunk.len()) > MAX_BYTES {
            Err(CompactError::LimitExceeded)
        } else {
            self.buffer.push_str(chunk);
            self.scan()
        };
        if let Err(error) = &result {
            self.error = Some(error.clone());
        }
        result
    }
    fn scan(&mut self) -> Result<(), CompactError> {
        const MARKER: &[u8] = b"<<";
        const NATIVE_MARKER: &[u8] = b"[TOOL_CALLS]";
        while self.cursor < self.buffer.len() {
            let bytes = self.buffer.as_bytes();
            let b = bytes[self.cursor];
            match &mut self.state {
                State::Text => {
                    let remaining = &bytes[self.cursor..];
                    if (remaining.len() < MARKER.len() && MARKER.starts_with(remaining))
                        || (remaining.len() < NATIVE_MARKER.len()
                            && NATIVE_MARKER.starts_with(remaining))
                    {
                        break;
                    }
                    if remaining.starts_with(MARKER) {
                        self.call_start = self.cursor;
                        self.cursor += MARKER.len();
                        self.frame = Frame::Brackets;
                        self.state = State::Name {
                            start: self.cursor,
                            prefixed: false,
                        };
                    } else if remaining.starts_with(NATIVE_MARKER) {
                        self.call_start = self.cursor;
                        self.cursor += NATIVE_MARKER.len();
                        self.frame = Frame::Native;
                        self.state = State::Name {
                            start: self.cursor,
                            prefixed: true,
                        };
                    } else {
                        self.cursor += 1;
                    }
                }
                State::Name { start, prefixed } => {
                    if name_char(b) {
                        self.cursor += 1;
                        if self.cursor - *start > 128 {
                            return Err(CompactError::LimitExceeded);
                        }
                    } else if (b.is_ascii_whitespace()
                        || b == b'{'
                        || (self.frame == Frame::Native && b == b'('))
                        && self.cursor > *start
                    {
                        self.name = self.buffer[*start..self.cursor].into();
                        if !*prefixed && self.name == "call" && b.is_ascii_whitespace() {
                            self.state = State::AfterCall;
                            continue;
                        }
                        if !self.tools.iter().any(|t| t.name == self.name) {
                            return Err(CompactError::UnknownTool(self.name.clone()));
                        }
                        self.state = State::BeforeJson;
                    } else {
                        return Err(CompactError::MalformedOutput("invalid call name".into()));
                    }
                }
                State::BeforeJson => {
                    if b.is_ascii_whitespace() {
                        self.cursor += 1;
                    } else if self.frame == Frame::Native && b == b'(' {
                        self.frame = Frame::Parenthesized;
                        self.cursor += 1;
                    } else if b == b'{' {
                        self.state = State::Json {
                            start: self.cursor,
                            stack: vec![b'{'],
                            quoted: false,
                            escaped: false,
                        };
                        self.cursor += 1;
                    } else {
                        return Err(CompactError::MalformedOutput(
                            "arguments must be a JSON object".into(),
                        ));
                    }
                }
                State::AfterCall => {
                    if b.is_ascii_whitespace() {
                        self.cursor += 1;
                    } else if b == b'{' {
                        if !self.tools.iter().any(|t| t.name == "call") {
                            return Err(CompactError::UnknownTool("call".into()));
                        }
                        self.state = State::BeforeJson;
                    } else if name_char(b) {
                        self.state = State::Name {
                            start: self.cursor,
                            prefixed: true,
                        };
                    } else {
                        return Err(CompactError::MalformedOutput("invalid call prefix".into()));
                    }
                }
                State::Json {
                    start,
                    stack,
                    quoted,
                    escaped,
                } => {
                    if *quoted {
                        if *escaped {
                            *escaped = false;
                        } else if b == b'\\' {
                            *escaped = true;
                        } else if b == b'"' {
                            *quoted = false;
                        }
                    } else {
                        match b {
                            b'"' => *quoted = true,
                            b'{' | b'[' => {
                                stack.push(b);
                                if stack.len() > MAX_DEPTH {
                                    return Err(CompactError::LimitExceeded);
                                }
                            }
                            b'}' | b']' => {
                                let expected = if b == b'}' { b'{' } else { b'[' };
                                if stack.pop() != Some(expected) {
                                    return Err(CompactError::MalformedOutput(
                                        "mismatched JSON delimiters".into(),
                                    ));
                                }
                                if stack.is_empty() {
                                    let parsed = serde_json::from_str::<UniqueValue>(
                                        &self.buffer[*start..=self.cursor],
                                    )
                                    .map_err(|e| {
                                        if e.to_string().contains("duplicate JSON key") {
                                            CompactError::InvalidArguments(
                                                "duplicate JSON key".into(),
                                            )
                                        } else {
                                            CompactError::MalformedOutput(
                                                "invalid JSON arguments".into(),
                                            )
                                        }
                                    })?;
                                    self.args = Some(parsed.0);
                                    if self.frame == Frame::Native {
                                        self.commit_call(self.cursor + 1)?;
                                        continue;
                                    }
                                    self.state = State::Close;
                                }
                            }
                            _ => {}
                        }
                    }
                    self.cursor += 1;
                }
                State::Close => {
                    if b.is_ascii_whitespace() {
                        self.cursor += 1;
                    } else if self.frame == Frame::Parenthesized && b == b')' {
                        self.commit_call(self.cursor + 1)?;
                    } else if self.frame == Frame::Brackets && b == b'>' {
                        if self.cursor + 1 == bytes.len() {
                            break;
                        }
                        if bytes[self.cursor + 1] != b'>' {
                            return Err(CompactError::MalformedOutput("expected >>".into()));
                        }
                        self.commit_call(self.cursor + 2)?;
                    } else {
                        return Err(CompactError::MalformedOutput(
                            "invalid delimiter after JSON object".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    fn commit_call(&mut self, end: usize) -> Result<(), CompactError> {
        let call = ToolCall {
            name: self.name.clone(),
            arguments: self.args.take().expect("parsed JSON"),
        };
        validate_call(&call, &self.tools)?;
        if self.calls.len() >= MAX_CALLS {
            return Err(CompactError::LimitExceeded);
        }
        self.calls.push(call);
        self.cursor = end;
        self.spans
            .push((self.call_start, end, self.frame != Frame::Brackets));
        self.state = State::Text;
        Ok(())
    }
    pub fn finish(self) -> Result<Vec<ToolCall>, CompactError> {
        Ok(self.finish_response()?.calls)
    }
    pub fn finish_response(self) -> Result<DecodedResponse, CompactError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !matches!(self.state, State::Text)
            || (self.cursor < self.buffer.len() && self.buffer.len() - self.cursor >= 2)
        {
            return Err(CompactError::MalformedOutput(
                "truncated call marker or arguments".into(),
            ));
        }
        let mut text = String::new();
        let mut previous = 0;
        let mut previous_native = false;
        for (start, end, native) in self.spans {
            check_native_suffix(previous_native, &self.buffer[previous..start])?;
            text.push_str(&self.buffer[previous..start]);
            previous = end;
            previous_native = native;
        }
        check_native_suffix(previous_native, &self.buffer[previous..])?;
        text.push_str(&self.buffer[previous..]);
        Ok(DecodedResponse {
            calls: self.calls,
            text,
        })
    }
}

fn check_native_suffix(native: bool, gap: &str) -> Result<(), CompactError> {
    if native
        && gap
            .trim_start()
            .chars()
            .next()
            .is_some_and(|c| matches!(c, '}' | ']' | ')' | '>'))
    {
        return Err(CompactError::MalformedOutput(
            "extra native call delimiter".into(),
        ));
    }
    Ok(())
}

pub fn decode_response(text: &str, tools: &[ToolDef]) -> Result<DecodedResponse, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.push(text)?;
    decoder.finish_response()
}

pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    Ok(decode_response(text, tools)?.calls)
}
