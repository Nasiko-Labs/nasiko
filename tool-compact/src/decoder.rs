use std::collections::{BTreeMap, BTreeSet};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};

use crate::codec::validate_tool_set;
use crate::{CompactError, ToolCall, ToolDef};

const OPEN_MARKER: &str = "<<call ";

/// Decode every compact call in a complete model response.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    decoder.feed(text)?;
    decoder.finish()
}

/// Render one validated-grammar call block without changing the JSON arguments.
pub fn render_call(name: &str, arguments: &Value) -> Result<String, CompactError> {
    if !arguments.is_object() {
        return Err(CompactError::InvalidArguments {
            tool: name.to_string(),
            reason: "arguments must be a JSON object".to_string(),
        });
    }
    let arguments = serde_json::to_string(arguments).map_err(|error| {
        CompactError::InvalidArguments {
            tool: name.to_string(),
            reason: error.to_string(),
        }
    })?;
    Ok(format!("<<call {name} {arguments}>>"))
}

/// Incremental decoder for compact call blocks split across arbitrary text chunks.
pub struct StreamDecoder {
    tools: BTreeMap<String, ToolDef>,
    state: State,
    pending: Vec<PendingCall>,
    failure: Option<CompactError>,
}

impl StreamDecoder {
    /// Create a decoder and reject ambiguous duplicate tool definitions up front.
    pub fn new(tools: &[ToolDef]) -> Result<Self, CompactError> {
        validate_tool_set(tools)?;
        Ok(Self {
            tools: tools
                .iter()
                .map(|tool| (tool.function.name.clone(), tool.clone()))
                .collect(),
            state: State::Text {
                marker_prefix: String::new(),
            },
            pending: Vec::new(),
            failure: None,
        })
    }

    /// Feed one response chunk. Calls are withheld until [`Self::finish`] validates the batch.
    pub fn feed(&mut self, chunk: &str) -> Result<(), CompactError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        for character in chunk.chars() {
            if let Err(error) = self.feed_character(character) {
                self.failure = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }

    /// Alias for callers that describe stream ingestion as pushing chunks.
    pub fn push(&mut self, chunk: &str) -> Result<(), CompactError> {
        self.feed(chunk)
    }

    /// Finish the stream, rejecting truncated syntax and validating every call atomically.
    pub fn finish(self) -> Result<Vec<ToolCall>, CompactError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        match &self.state {
            State::Text { marker_prefix } if marker_prefix.len() < 2 => {}
            State::Text { .. }
            | State::Name { .. }
            | State::BeforeArguments { .. }
            | State::Arguments { .. }
            | State::Closing { .. } => return Err(CompactError::TruncatedCall),
        }
        self.pending
            .iter()
            .map(|pending| self.validate_call(pending))
            .collect()
    }

    fn feed_character(&mut self, character: char) -> Result<(), CompactError> {
        let state = std::mem::replace(
            &mut self.state,
            State::Text {
                marker_prefix: String::new(),
            },
        );
        self.state = match state {
            State::Text { mut marker_prefix } => {
                marker_prefix.push(character);
                while !marker_prefix.is_empty() && !OPEN_MARKER.starts_with(&marker_prefix) {
                    marker_prefix.remove(0);
                }
                if marker_prefix == OPEN_MARKER {
                    State::Name {
                        name: String::new(),
                    }
                } else {
                    State::Text { marker_prefix }
                }
            }
            State::Name { mut name } => {
                if character.is_whitespace() {
                    if name.is_empty() {
                        return Err(CompactError::MalformedSyntax(
                            "tool name is empty".to_string(),
                        ));
                    }
                    State::BeforeArguments { name }
                } else if character.is_control() || matches!(character, '{' | '<' | '>') {
                    return Err(CompactError::MalformedSyntax(
                        "invalid character in tool name".to_string(),
                    ));
                } else {
                    name.push(character);
                    State::Name { name }
                }
            }
            State::BeforeArguments { name } => {
                if character.is_whitespace() {
                    State::BeforeArguments { name }
                } else if character == '{' {
                    State::Arguments {
                        name,
                        json: "{".to_string(),
                        closing_delimiters: vec!['}'],
                        in_string: false,
                        escaped: false,
                    }
                } else {
                    return Err(CompactError::MalformedSyntax(
                        "arguments must start with a JSON object".to_string(),
                    ));
                }
            }
            State::Arguments {
                name,
                mut json,
                mut closing_delimiters,
                mut in_string,
                mut escaped,
            } => {
                json.push(character);
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if character == '"' {
                        in_string = false;
                    }
                } else {
                    match character {
                        '"' => in_string = true,
                        '{' => closing_delimiters.push('}'),
                        '[' => closing_delimiters.push(']'),
                        '}' | ']' => {
                            if closing_delimiters.pop() != Some(character) {
                                return Err(CompactError::MalformedSyntax(
                                    "mismatched JSON delimiter".to_string(),
                                ));
                            }
                        }
                        _ => {}
                    }
                }
                if closing_delimiters.is_empty() {
                    State::Closing {
                        name,
                        json,
                        markers_seen: 0,
                    }
                } else {
                    State::Arguments {
                        name,
                        json,
                        closing_delimiters,
                        in_string,
                        escaped,
                    }
                }
            }
            State::Closing {
                name,
                json,
                markers_seen,
            } => {
                if character != '>' {
                    return Err(CompactError::MalformedSyntax(
                        "call must end with '>>' immediately after the arguments".to_string(),
                    ));
                }
                if markers_seen == 0 {
                    State::Closing {
                        name,
                        json,
                        markers_seen: 1,
                    }
                } else {
                    self.pending.push(PendingCall { name, json });
                    State::Text {
                        marker_prefix: String::new(),
                    }
                }
            }
        };
        Ok(())
    }

    fn validate_call(&self, pending: &PendingCall) -> Result<ToolCall, CompactError> {
        let tool = self
            .tools
            .get(&pending.name)
            .ok_or_else(|| CompactError::UnknownTool(pending.name.clone()))?;
        let arguments = parse_unique_json(&pending.json).map_err(|reason| {
            CompactError::InvalidArguments {
                tool: pending.name.clone(),
                reason,
            }
        })?;
        if !arguments.is_object() {
            return Err(CompactError::InvalidArguments {
                tool: pending.name.clone(),
                reason: "arguments must be a JSON object".to_string(),
            });
        }
        if let Some(schema) = &tool.function.parameters {
            let validator = jsonschema::options()
                .offline()
                .should_validate_formats(true)
                .build(schema)
                .map_err(|error| CompactError::InvalidArguments {
                    tool: pending.name.clone(),
                    reason: format!("tool schema is invalid: {error}"),
                })?;
            if !validator.is_valid(&arguments) {
                return Err(CompactError::InvalidArguments {
                    tool: pending.name.clone(),
                    reason: "arguments do not satisfy the original JSON Schema".to_string(),
                });
            }
        }
        Ok(ToolCall {
            name: pending.name.clone(),
            arguments,
        })
    }
}

struct PendingCall {
    name: String,
    json: String,
}

enum State {
    Text {
        marker_prefix: String,
    },
    Name {
        name: String,
    },
    BeforeArguments {
        name: String,
    },
    Arguments {
        name: String,
        json: String,
        closing_delimiters: Vec<char>,
        in_string: bool,
        escaped: bool,
    },
    Closing {
        name: String,
        json: String,
        markers_seen: u8,
    },
}

fn parse_unique_json(input: &str) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let UniqueValue(value) = UniqueValue::deserialize(&mut deserializer)
        .map_err(|error| format!("malformed JSON or duplicate object key: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("trailing JSON input: {error}"))?;
    Ok(value)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueValueVisitor)
    }
}

struct UniqueValueVisitor;

impl<'de> Visitor<'de> for UniqueValueVisitor {
    type Value = UniqueValue;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(UniqueValue(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(UniqueValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate object key '{key}'")));
            }
            let UniqueValue(value) = object.next_value()?;
            values.insert(key, value);
        }
        Ok(UniqueValue(Value::Object(values)))
    }
}

