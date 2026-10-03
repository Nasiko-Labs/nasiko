//! Decoding of compact tool calls back into OpenAI-shaped [`ToolCall`]s.
//!
//! P1 renders tool definitions compactly in the prompt and lets the model answer with
//! compact calls of the form:
//!
//! ```text
//! <<call name {json args}>>
//! ```
//!
//! e.g. `<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>`.
//! This module is the decode half of that pipeline: it turns such calls back into
//! standard OpenAI tool calls and strips them from the visible text, so a client never
//! receives the compact representation.
//!
//! # Fail-closed
//!
//! Model output is untrusted, so the decoder is strict by design and never guesses:
//!
//! * an unknown tool name → [`DecodeError::UnknownTool`]
//! * arguments that are not valid JSON, or that violate the tool's schema (missing
//!   required field, wrong type, invalid enum, unexpected field under
//!   `additionalProperties: false`, …) → [`DecodeError::InvalidArguments`]
//! * a committed call with broken syntax → [`DecodeError::MalformedCall`]
//! * a call still open at end of input → [`DecodeError::UnterminatedCall`]
//!
//! Invalid values are never converted into valid ones, missing fields are never
//! invented, and malformed input never panics. Errors are deterministic: the same
//! input always yields the same error, with a byte offset for debugging.
//!
//! # Grammar
//!
//! The marker is `<<call` followed by at least one whitespace character, then the tool
//! name (`[A-Za-z0-9_-]+`, the OpenAI function-name charset), then the JSON arguments
//! object (the braces are mandatory, `{}` for no arguments), then optional whitespace
//! and the closing `>>`. Any run of whitespace may separate the parts. Normal text is
//! allowed before, between and after calls; a `<<` that never becomes a full marker is
//! literal text. Once `<<call` is matched, the call is *committed*: anything wrong
//! after that point is an error, not prose. A `>>` inside a JSON string is string
//! content, never the closing marker.
//!
//! # Supported schema subset
//!
//! Validation enforces `type` (including arrays such as `["string", "null"]`), `enum`,
//! `const`, `required`, recursive `properties`, `items`, `additionalProperties`, and
//! `allOf`/`anyOf`/`oneOf`. Unsupported validation keywords fail explicitly rather
//! than being silently skipped; only annotation keywords such as `description` and
//! `title` are ignored — see [`validate`].
//!
//! # Streaming
//!
//! [`StreamDecoder`] feeds the same state machine chunk by chunk: the marker, the tool
//! name, the JSON, a string, an escape or the closing `>>` may split across chunks
//! arbitrarily. For the same complete input, [`decode_calls`] and a `StreamDecoder`
//! run over any chunking of it produce identical calls and identical errors.

mod parse;
mod validate;

pub use parse::{StreamDecoder, Streamed};

use crate::ir::chat::{ToolCall, ToolDef};

/// Every way decoding can fail.
///
/// The variants are the error categories; [`DecodeError::category`] gives their
/// stable string labels.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The model named a tool that is not in the request's tool list.
    #[error("unknown tool '{tool}'")]
    UnknownTool {
        /// The tool name as it appeared in the compact call.
        tool: String,
    },
    /// The arguments are not valid JSON or violate the tool's schema.
    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments {
        /// The (known) tool whose arguments were rejected.
        tool: String,
        /// What was wrong — deterministic and free of untrusted payload data.
        reason: String,
    },
    /// A committed `<<call …` has broken syntax.
    #[error("malformed compact tool call at byte {offset}: {reason}")]
    MalformedCall {
        /// Byte offset in the input where the problem was detected.
        offset: usize,
        /// What was wrong.
        reason: String,
    },
    /// A call was still open when the input (or stream) ended.
    #[error("unterminated compact tool call at byte {offset}: {reason}")]
    UnterminatedCall {
        /// Byte offset in the input where the open call began.
        offset: usize,
        /// What was still missing.
        reason: String,
    },
}

impl DecodeError {
    /// Stable category label, for telemetry and tests.
    pub fn category(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::MalformedCall { .. } => "malformed_call",
            Self::UnterminatedCall { .. } => "unterminated_call",
        }
    }
}

/// Decode every compact tool call in `text` into OpenAI-shaped [`ToolCall`]s.
///
/// Text before, between and after calls is the visible answer and is not returned;
/// only the calls are, in the order they appear, with deterministic ids `call_1`,
/// `call_2`, …. Each call's `arguments` is kept as the exact JSON text the model
/// produced. A response with no compact calls decodes to `[]`.
///
/// Fails closed: the first malformed, unknown or schema-violating call aborts the
/// whole decode with a [`DecodeError`] — earlier, valid calls are discarded rather
/// than half-delivered.
pub fn decode_calls(text: &str, tools: &[ToolDef]) -> Result<Vec<ToolCall>, DecodeError> {
    // One chunk + finish runs the exact same state machine as streaming, so the two
    // APIs cannot disagree about calls or errors for the same complete input.
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = decoder.push_chunk(text)?.calls;
    calls.extend(decoder.finish()?.calls);
    Ok(calls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::FunctionDef;
    use serde_json::{Value, json};

    /// The shared fixture set: one tool per schema feature the decoder must handle.
    pub(super) fn tool_defs() -> Vec<ToolDef> {
        vec![
            tool(
                "create_calendar_event",
                json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string" },
                        "duration_min": { "type": "integer" },
                        "visibility": { "type": "string", "enum": ["public", "private"] },
                        "attendees": { "type": "array", "items": { "type": "string" } },
                        "location": {
                            "type": "object",
                            "properties": { "name": { "type": "string" } },
                            "required": ["name"],
                        },
                    },
                    "required": ["title", "start"],
                }),
            ),
            tool(
                "send_email",
                json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" },
                        "body": { "type": "string" },
                    },
                    "required": ["to", "subject", "body"],
                }),
            ),
            // No schema at all: only JSON well-formedness can be checked.
            tool("noop", Value::Null),
            tool(
                "strict",
                json!({
                    "type": "object",
                    "properties": { "a": { "type": "string" } },
                    "required": ["a"],
                    "additionalProperties": false,
                }),
            ),
        ]
    }

    pub(super) fn tool(name: &str, parameters: Value) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: None,
                parameters: Some(parameters),
            },
            extra: Default::default(),
        }
    }

    pub(super) fn serialize_calls(calls: &[ToolCall]) -> Vec<Value> {
        calls
            .iter()
            .map(|c| serde_json::to_value(c).expect("ToolCall serializes"))
            .collect()
    }

    // ── valid cases ──────────────────────────────────────────────────────────

    #[test]
    fn plain_text_decodes_to_no_calls() {
        let text = "Hello, how are you?";
        assert!(decode_calls(text, &tool_defs()).unwrap().is_empty());
    }

    #[test]
    fn one_call_decodes_to_openai_shape() {
        let text = "<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
        let calls = decode_calls(text, &tool_defs()).unwrap();

        let v = serde_json::to_value(&calls).unwrap();
        assert_eq!(v.as_array().map(Vec::len), Some(1));
        assert_eq!(v[0]["id"], "call_1");
        assert_eq!(v[0]["type"], "function");
        assert_eq!(v[0]["function"]["name"], "create_calendar_event");
        // Arguments kept as the exact JSON text the model produced.
        assert_eq!(
            v[0]["function"]["arguments"],
            "{\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}"
        );
    }

    #[test]
    fn multiple_calls_get_sequential_ids_and_keep_order() {
        let text = "One.\n<<call create_calendar_event {\"title\":\"A\",\"start\":\"s\"}>>\nTwo.\n<<call send_email {\"to\":[\"x@y.z\"],\"subject\":\"Hi\",\"body\":\"Done\"}>>\nThree.";
        let calls = decode_calls(text, &tool_defs()).unwrap();

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(calls[1].id, "call_2");
        assert_eq!(calls[1].function.name, "send_email");
    }

    #[test]
    fn nested_objects_arrays_and_enums_decode() {
        let text = "<<call create_calendar_event {\"title\":\"设计 review\",\"start\":\"2026-10-05T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\",\"attendees\":[\"a@b.c\",\"d@e.f\"],\"location\":{\"name\":\"Room 4 >> 2\"}}>>";
        let calls = decode_calls(text, &tool_defs()).unwrap();

        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "设计 review");
        assert_eq!(args["duration_min"], 30);
        assert_eq!(args["visibility"], "private");
        assert_eq!(args["attendees"], json!(["a@b.c", "d@e.f"]));
        assert_eq!(args["location"]["name"], "Room 4 >> 2");
    }

    #[test]
    fn optional_fields_may_be_absent_and_unicode_survives() {
        let text =
            "<<call create_calendar_event {\"title\":\"Café ☕ >> meeting\",\"start\":\"s\"}>>";
        let calls = decode_calls(text, &tool_defs()).unwrap();
        assert_eq!(calls.len(), 1);
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert!(args.get("duration_min").is_none());
        assert!(args.get("visibility").is_none());
    }

    #[test]
    fn a_tool_without_schema_accepts_any_json_object() {
        let text = "<<call noop {\"anything\":[1,true,null,\"x\"]}>>";
        let calls = decode_calls(text, &tool_defs()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "noop");
    }

    #[test]
    fn whitespace_between_all_parts_is_tolerated() {
        let text =
            "<<call\t create_calendar_event\n  { \"title\" : \"A\" , \"start\" : \"s\" }\n>>";
        let calls = decode_calls(text, &tool_defs()).unwrap();
        assert_eq!(calls.len(), 1);
    }

    // ── error cases ──────────────────────────────────────────────────────────

    #[test]
    fn unknown_tool_fails_closed() {
        let text = "<<call unknown_tool {\"x\":1}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "unknown_tool");
        assert!(err.to_string().contains("unknown_tool"));
    }

    #[test]
    fn missing_required_field_fails() {
        let text = "<<call create_calendar_event {\"duration_min\":30}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "invalid_arguments");
        assert!(err.to_string().contains("missing required field"), "{err}");
    }

    #[test]
    fn wrong_argument_type_fails() {
        let text = "<<call create_calendar_event {\"title\":42,\"start\":\"s\"}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "invalid_arguments");
        assert!(err.to_string().contains("string") && err.to_string().contains("number"));
    }

    #[test]
    fn invalid_enum_value_fails_without_being_corrected() {
        let text = "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\",\"visibility\":\"secret\"}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "invalid_arguments");
        assert!(err.to_string().contains("public") && err.to_string().contains("private"));
    }

    #[test]
    fn nested_required_field_fails() {
        let text = "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\",\"location\":{\"room\":\"4B\"}}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "invalid_arguments");
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn malformed_json_fails() {
        for args in [
            "{\"title\":}",
            "{\"title\" \"T\",\"start\":\"s\"}",
            "{\"a\":1,}",
        ] {
            let text = format!("<<call create_calendar_event {args}>>");
            let err = decode_calls(&text, &tool_defs()).unwrap_err();
            assert_eq!(err.category(), "invalid_arguments", "{text}");
        }
    }

    #[test]
    fn invalid_escape_sequence_fails() {
        let text =
            "<<call send_email {\"to\":[\"x\"],\"subject\":\"s\",\"body\":\"bad \\x escape\"}>>";
        let err = decode_calls(text, &tool_defs()).unwrap_err();
        assert_eq!(err.category(), "invalid_arguments");
    }

    #[test]
    fn empty_and_invalid_tool_names_fail() {
        for text in [
            "<<call {\"a\":1}>>",
            "<<call  {\"a\":1}>>",
            "<<call foo.bar {\"a\":1}>>",
            "<<call fo(o) {\"a\":1}>>",
        ] {
            let err = decode_calls(text, &tool_defs()).unwrap_err();
            assert_eq!(err.category(), "malformed_call", "{text}");
        }
    }

    #[test]
    fn malformed_compact_syntax_fails() {
        for text in [
            "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"} >x", // junk before >>
            "<<call create_calendar_event x",                                    // no `{`
            "<<call create_calendar_event {\"title\":\"T\"} } >>",               // junk after args
        ] {
            let err = decode_calls(text, &tool_defs()).unwrap_err();
            assert_eq!(err.category(), "malformed_call", "{text}");
        }
    }

    #[test]
    fn unterminated_call_fails() {
        for text in [
            "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}", // no >>
            "<<call create_calendar_event {\"title\":\"T\"",                  // open JSON
            "<<call create_calendar_event {\"title\":\"T\\\"}}",              // open string
            "<<call create_calendar_event",                                   // no args at all
            "<<call create_cal",                                              // mid tool name
            "<<call",                                                         // bare marker
            "<<call create_calendar_event {\"title\":\"T\"} >",               // dangling >
        ] {
            let err = decode_calls(text, &tool_defs()).unwrap_err();
            assert_eq!(err.category(), "unterminated_call", "{text}");
        }
    }

    #[test]
    fn errors_carry_deterministic_byte_offsets() {
        let first = decode_calls("x<<call nope {\"a\":1}>>", &tool_defs()).unwrap_err();
        let second = decode_calls("x<<call nope {\"a\":1}>>", &tool_defs()).unwrap_err();
        assert_eq!(first, second, "same input must give the same error");
    }

    // ── differential streaming: decode_calls == StreamDecoder over any chunking ──

    /// Deterministic pseudo-random char-boundary-safe chunking (no RNG dependency).
    fn lcg_chunks(text: &str, seed: u64, max_len: usize) -> Vec<&str> {
        let mut state = seed | 1;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize % max_len + 1
        };
        let mut chunks = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut take = next().min(rest.len());
            while !rest.is_char_boundary(take) {
                take += 1;
            }
            let (chunk, tail) = rest.split_at(take);
            chunks.push(chunk);
            rest = tail;
        }
        chunks
    }

    /// One chunk per character (never splits a multibyte character).
    fn per_char(text: &str) -> Vec<&str> {
        text.char_indices()
            .map(|(i, c)| &text[i..i + c.len_utf8()])
            .collect()
    }

    fn fixed_chunks(text: &str, size: usize) -> Vec<&str> {
        let mut chunks = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut take = size.min(rest.len());
            while !rest.is_char_boundary(take) {
                take += 1;
            }
            let (chunk, tail) = rest.split_at(take);
            chunks.push(chunk);
            rest = tail;
        }
        chunks
    }

    /// Feed `text` through a `StreamDecoder` in the given chunks and require the same
    /// calls and the same error as [`decode_calls`].
    fn assert_chunking_matches(text: &str, chunks: Vec<&str>, tools: &[ToolDef]) {
        let expected = decode_calls(text, tools);

        let mut decoder = StreamDecoder::new(tools);
        let mut calls = Vec::new();
        let mut first_err = None;
        for chunk in chunks {
            match decoder.push_chunk(chunk) {
                Ok(out) => calls.extend(out.calls),
                Err(e) => {
                    first_err = Some(e);
                    break;
                }
            }
        }
        if first_err.is_none() {
            first_err = decoder.finish().err();
        }

        match (&expected, first_err) {
            (Ok(expected), None) => assert_eq!(
                serialize_calls(expected),
                serialize_calls(&calls),
                "streaming diverged from decode_calls for {text:?}"
            ),
            (Err(expected), Some(err)) => assert_eq!(*expected, err, "for {text:?}"),
            (Ok(_), Some(err)) => panic!("stream errored where decode_calls succeeded: {err:?}"),
            (Err(expected), None) => {
                panic!("stream succeeded where decode_calls errored: {expected:?}")
            }
        }
    }

    /// Run `text` through every chunking strategy and require identical results.
    fn assert_streaming_matches(text: &str, tools: &[ToolDef]) {
        assert_chunking_matches(text, vec![text], tools);
        assert_chunking_matches(text, per_char(text), tools);
        for size in [2, 3, 5, 7, 11, 16] {
            assert_chunking_matches(text, fixed_chunks(text, size), tools);
        }
        for seed in [1, 42, 1234] {
            assert_chunking_matches(text, lcg_chunks(text, seed, 9), tools);
        }
    }

    #[test]
    fn streaming_matches_decode_calls_for_every_valid_shape() {
        let tools = tool_defs();
        let cases = [
            "plain text, no calls",
            "<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>",
            "Hello.\n<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>\n<<call send_email {\"to\":[\"test@example.com\"],\"subject\":\"Hello\",\"body\":\"Done\"}>>\nThanks.",
            "<<ca",
        ];
        for text in cases {
            assert_streaming_matches(text, &tools);
        }
    }

    #[test]
    fn streaming_matches_decode_calls_for_error_cases() {
        let tools = tool_defs();
        let cases = [
            "<<call unknown_tool {\"x\":1}>>",
            "<<call create_calendar_event {\"duration_min\":30}>>",
            "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\",\"visibility\":\"secret\"}>>",
            "<<call create_calendar_event {\"title\":}>>",
            "<<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}",
            "<<call create_calendar_event {\"title\":\"T\"} >",
            "<<call foo.bar {\"a\":1}>>",
            "<<call {\"a\":1}>>",
        ];
        for text in cases {
            assert_streaming_matches(text, &tools);
        }
    }

    #[test]
    fn the_briefs_split_examples_decode_exactly_like_the_full_text() {
        let tools = tool_defs();

        // Marker + tool name + JSON string split across four chunks.
        let full = "<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>";
        let expected = decode_calls(full, &tools).unwrap();
        let streamed = {
            let mut d = StreamDecoder::new(&tools);
            let mut calls = Vec::new();
            for chunk in [
                "<<ca",
                "ll create_calendar_event {\"title\":\"Ret",
                "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
                ">",
            ] {
                calls.extend(d.push_chunk(chunk).unwrap().calls);
            }
            calls.extend(d.finish().unwrap().calls);
            calls
        };
        assert_eq!(serialize_calls(&expected), serialize_calls(&streamed));

        // Close marker split between the two `>`.
        let streamed = {
            let mut d = StreamDecoder::new(&tools);
            let mut calls = Vec::new();
            for chunk in [
                "<<",
                "call create_calendar_event ",
                "{\"title\":\"Meeting\"",
                ",\"start\":\"2026-10-05T10:00:00+05:30\"}",
                ">",
                ">",
            ] {
                calls.extend(d.push_chunk(chunk).unwrap().calls);
            }
            calls.extend(d.finish().unwrap().calls);
            calls
        };
        assert_eq!(
            serialize_calls(&decode_calls("<<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>", &tools).unwrap()),
            serialize_calls(&streamed)
        );
    }

    #[test]
    fn streamed_text_never_contains_the_compact_representation() {
        let tools = tool_defs();
        let text = "Before <<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}>> middle <<call send_email {\"to\":[\"x\"],\"subject\":\"s\",\"body\":\"b\"}>> after";
        let mut decoder = StreamDecoder::new(&tools);
        let mut streamed_text = String::new();
        for chunk in fixed_chunks(text, 5) {
            streamed_text.push_str(&decoder.push_chunk(chunk).unwrap().text);
        }
        streamed_text.push_str(&decoder.finish().unwrap().text);

        assert_eq!(
            streamed_text, "Before  middle  after",
            "compact calls must be stripped from the visible text"
        );
    }

    #[test]
    fn a_lone_lookalike_marker_passes_through_as_text() {
        let tools = tool_defs();
        // `<<` that never becomes `<<call` is prose, not a call.
        let text = "a << b <<calls are made <<c";
        assert!(decode_calls(text, &tools).unwrap().is_empty());
        let mut decoder = StreamDecoder::new(&tools);
        let mut streamed_text = String::new();
        for chunk in per_char(text) {
            streamed_text.push_str(&decoder.push_chunk(chunk).unwrap().text);
        }
        streamed_text.push_str(&decoder.finish().unwrap().text);
        assert_eq!(streamed_text, text);
    }

    #[test]
    fn malformed_output_never_panics() {
        let tools = tool_defs();
        let corpus = [
            "",
            "<",
            "<<",
            "<<<",
            "<<c",
            "<<ca",
            "<<cal",
            "<<call",
            "<<call ",
            "<<call{",
            "<<call foo",
            "<<call foo ",
            "<<call foo {",
            "<<call foo {}}>>",
            "<<call foo {}>",
            "<<call foo {} >>",
            "<<call {}>>",
            "<<call foo bar",
            "<<call foo bar {\"a\":1}>",
            "<<call foo {\"a\":}>>",
            "<<call foo {\"a\" 1}>>",
            "<<call foo [1,2]>>",
            "<<call foo {\"a\":\"\\\\\"}>> extra",
            "<<call foo {\"a\":1}>>x",
            "text <<call foo {\"a\":1}>>",
            "<<call 🦀 {}>>",
            "<<call foo {\"🦀\": [1, {\"b\": null}]}>>",
            ">>>> <<<<<<>>>>",
            "<<call call call",
            "<<call foo {\"a\":1} > >",
            "<<call foo {\"a\":1} >>>",
            "<<call foo {} >> more >>",
            "<<call foo {\"a\":\"unterminated",
            "<<call foo {\"a\":\"\\\\\"}",
            "\u{1F600}<<",
            "<<\u{1F600}",
        ];
        for text in corpus {
            // Whole-text decode: any panic fails the test.
            let _ = decode_calls(text, &tools);
            // And the same text streamed one character at a time.
            let mut decoder = StreamDecoder::new(&tools);
            for chunk in per_char(text) {
                let _ = decoder.push_chunk(chunk);
            }
            let _ = decoder.finish();
        }
    }

    #[test]
    fn the_decoder_is_poisoned_after_an_error() {
        let tools = tool_defs();
        let mut decoder = StreamDecoder::new(&tools);
        let err = decoder
            .push_chunk("<<call unknown_tool {\"a\":1}>>")
            .unwrap_err();
        assert_eq!(err.category(), "unknown_tool");

        // Every later interaction returns the same error; nothing more is decoded.
        let next = decoder.push_chunk("hello").unwrap_err();
        assert_eq!(next, err);
        assert_eq!(decoder.finish().unwrap_err(), err);
    }

    #[test]
    fn empty_tool_list_rejects_every_call() {
        let err = decode_calls("<<call anything {}>>", &[]).unwrap_err();
        assert_eq!(err.category(), "unknown_tool");
    }
}
