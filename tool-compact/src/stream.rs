//! Incremental decoding of compact tool calls.
//!
//! [`StreamDecoder`] joins the syntax layer ([`crate::grammar`]'s scanner) to the
//! meaning layer: each complete call is resolved against the tool list, its arguments
//! are parsed with a real JSON parser and then validated against the tool's original
//! schema ([`crate::validate`]). Text outside calls is passed through untouched.
//!
//! # Checks, in order
//!
//! 1. the tool name must match a tool exactly, else [`CompactError::UnknownTool`];
//! 2. the arguments must be valid JSON, else [`CompactError::InvalidSyntax`];
//! 3. the arguments must not repeat a key, which `serde_json` would otherwise resolve
//!    silently to the last value, else [`CompactError::InvalidArguments`];
//! 4. the arguments must satisfy the tool's schema, else
//!    [`CompactError::InvalidArguments`]. A tool without `parameters` takes exactly `{}`.
//!
//! # Fail closed
//!
//! The first error poisons the decoder: every later [`StreamDecoder::push`] and
//! [`StreamDecoder::finish`] returns that same error. A decoder never yields a call it
//! has not fully validated, and never guesses a name or repairs an argument.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::error::{CompactError, Result};
use crate::grammar::{Scanner, Token};
use crate::types::{ToolCall, ToolDef};
use crate::validate::validate_arguments;

/// Output of [`StreamDecoder::push`], in the order it occurred in the model output.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Text outside any call. Consecutive text may arrive as several events.
    Text(String),
    /// A complete call that passed every check.
    Call(ToolCall),
}

/// Incremental decoder for model output written in the compact call grammar.
///
/// Feed chunks with [`push`](Self::push) as they arrive, then call
/// [`finish`](Self::finish). Chunk boundaries may fall anywhere, including inside a
/// marker, a tool name or a JSON string.
pub struct StreamDecoder {
    tools: Vec<ToolDef>,
    scanner: Scanner,
    calls: Vec<ToolCall>,
    error: Option<CompactError>,
}

impl StreamDecoder {
    pub fn new(tools: &[ToolDef]) -> Self {
        Self {
            tools: tools.to_vec(),
            scanner: Scanner::new(),
            calls: Vec::new(),
            error: None,
        }
    }

    /// Feed the next chunk of model output. Returns the text and validated calls it
    /// completed. After an error, every later call returns the same error.
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        let result = self
            .scanner
            .push(chunk)
            .and_then(|tokens| self.resolve(tokens));
        self.record(result)
    }

    /// Signal end of output and return every call, in order. Fails with
    /// [`CompactError::IncompleteStream`] if the output ended inside a call.
    pub fn finish(mut self) -> Result<Vec<ToolCall>> {
        if let Some(e) = self.error {
            return Err(e);
        }
        let tokens = self.scanner.finish()?;
        self.resolve(tokens)?;
        Ok(self.calls)
    }

    fn record(&mut self, result: Result<Vec<StreamEvent>>) -> Result<Vec<StreamEvent>> {
        if let Err(e) = &result {
            self.error = Some(e.clone());
        }
        result
    }

    fn resolve(&mut self, tokens: Vec<Token>) -> Result<Vec<StreamEvent>> {
        let mut events = Vec::with_capacity(tokens.len());
        for token in tokens {
            match token {
                Token::Text(text) => events.push(StreamEvent::Text(text)),
                Token::Call { name, args } => {
                    let call = self.resolve_call(name, &args)?;
                    self.calls.push(call.clone());
                    events.push(StreamEvent::Call(call));
                }
            }
        }
        Ok(events)
    }

    fn resolve_call(&self, name: String, args: &str) -> Result<ToolCall> {
        let Some(tool) = self.tools.iter().find(|t| t.name == name) else {
            return Err(CompactError::UnknownTool(name));
        };
        let arguments = parse_arguments(&name, args)?;
        match &tool.parameters {
            Some(schema) => {
                validate_arguments(schema, &arguments).map_err(|e| with_tool(e, &name))?
            }
            None if arguments.as_object().is_some_and(Map::is_empty) => {}
            None => {
                return Err(CompactError::InvalidArguments {
                    tool: name,
                    reason: "tool takes no arguments; expected {}".to_string(),
                });
            }
        }
        Ok(ToolCall { name, arguments })
    }
}

/// Parse an argument object, rejecting malformed JSON and duplicate keys.
fn parse_arguments(tool: &str, args: &str) -> Result<Value> {
    if let Err(e) = serde_json::from_str::<Value>(args) {
        return Err(CompactError::InvalidSyntax(format!(
            "arguments of '{tool}' are not valid JSON: {e}"
        )));
    }
    serde_json::from_str::<StrictValue>(args)
        .map(|v| v.0)
        .map_err(|e| CompactError::InvalidArguments {
            tool: tool.to_string(),
            reason: e.to_string(),
        })
}

/// The validator does not know which tool it is checking; attach the name here.
fn with_tool(error: CompactError, name: &str) -> CompactError {
    match error {
        CompactError::InvalidArguments { reason, .. } => CompactError::InvalidArguments {
            tool: name.to_string(),
            reason,
        },
        CompactError::InvalidSchema { reason, .. } => CompactError::InvalidSchema {
            tool: name.to_string(),
            reason,
        },
        other => other,
    }
}

/// A JSON value parsed without `serde_json`'s silent last-key-wins on duplicates.
struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(StrictValue)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_bool<E>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> std::result::Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("number is not finite"))
    }

    fn visit_str<E>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(v)) = seq.next_element()? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key '{key}'")));
            }
            let StrictValue(value) = map.next_value()?;
            object.insert(key, value);
        }
        Ok(Value::Object(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "start": {"type": "string", "format": "date-time"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            ToolDef {
                name: "get_time".into(),
                description: None,
                parameters: None,
            },
        ]
    }

    fn feed(chunks: &[&str]) -> (Vec<StreamEvent>, Result<Vec<ToolCall>>) {
        let mut d = StreamDecoder::new(&tools());
        let mut events = Vec::new();
        for chunk in chunks {
            match d.push(chunk) {
                Ok(e) => events.extend(e),
                Err(e) => return (events, Err(e)),
            }
        }
        (events, d.finish())
    }

    #[test]
    fn decoder_case_marker_split_across_chunks() {
        let (_, calls) = feed(&[
            "<<ca",
            "ll create_calendar_event {\"title\":\"Ret",
            "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
            ">",
        ]);
        assert_eq!(
            calls.unwrap(),
            vec![ToolCall {
                name: "create_calendar_event".into(),
                arguments: json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30"}),
            }]
        );
    }

    #[test]
    fn events_keep_text_and_calls_in_order() {
        let (events, calls) = feed(&["Okay. <<call get_time {}>> Done."]);
        let call = ToolCall {
            name: "get_time".into(),
            arguments: json!({}),
        };
        assert_eq!(
            events,
            vec![
                StreamEvent::Text("Okay. ".into()),
                StreamEvent::Call(call.clone()),
                StreamEvent::Text(" Done.".into()),
            ]
        );
        assert_eq!(calls.unwrap(), vec![call]);
    }

    #[test]
    fn unknown_tool_is_checked_before_arguments() {
        let (_, calls) = feed(&["<<call delete_everything {not json}>>"]);
        assert_eq!(
            calls,
            Err(CompactError::UnknownTool("delete_everything".into()))
        );
    }

    #[test]
    fn invalid_arguments_carry_the_tool_name() {
        let (_, calls) = feed(&[
            "<<call create_calendar_event {\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"secret\"}>>",
        ]);
        match calls {
            Err(CompactError::InvalidArguments { tool, .. }) => {
                assert_eq!(tool, "create_calendar_event")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_a_syntax_error() {
        let (_, calls) = feed(&["<<call get_time {\"a\":}>>"]);
        assert!(matches!(calls, Err(CompactError::InvalidSyntax(_))));
    }

    #[test]
    fn duplicate_keys_are_rejected_not_resolved() {
        let (_, calls) = feed(&[
            "<<call create_calendar_event {\"title\":\"A\",\"title\":\"B\",\"start\":\"2026-10-05T15:00:00Z\"}>>",
        ]);
        match calls {
            Err(CompactError::InvalidArguments { reason, .. }) => {
                assert!(reason.contains("duplicate key 'title'"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tool_without_parameters_takes_only_empty_object() {
        assert!(feed(&["<<call get_time {}>>"]).1.is_ok());
        assert!(matches!(
            feed(&["<<call get_time {\"tz\":\"IST\"}>>"]).1,
            Err(CompactError::InvalidArguments { .. })
        ));
    }

    #[test]
    fn errors_poison_the_decoder() {
        let mut d = StreamDecoder::new(&tools());
        let first = d.push("<<call nope {}>>").unwrap_err();
        assert_eq!(d.push("<<call get_time {}>>"), Err(first.clone()));
        assert_eq!(d.finish(), Err(first));
    }

    #[test]
    fn unfinished_call_fails_on_finish() {
        let (events, calls) = feed(&["text <<call get_time {"]);
        assert_eq!(events, vec![StreamEvent::Text("text ".into())]);
        assert_eq!(calls, Err(CompactError::IncompleteStream));
    }
}
