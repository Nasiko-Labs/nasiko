//! Compact function-tool schemas and a fail-closed decoder for the calls a model
//! writes back.
//!
//! Tool JSON Schema is verbose relative to the information a model needs to pick
//! a tool and fill its arguments. This crate turns a tool list into a short
//! prompt and parses `<<call name {json}>>` back into OpenAI-shaped tool calls,
//! checked against the **original** schema.
//!
//! It does not read the environment, perform I/O, or know about any model
//! provider. [`nasiko_compress`](https://docs.rs/nasiko-compress) is a different
//! problem: it shortens tool *results* and returns the original text on any
//! doubt. Compacting a schema that way would drop constraints, so this crate
//! does not share that pipeline. If a schema cannot be represented, compaction
//! is skipped for the whole list.
//!
//! # Grammar
//!
//! Chosen over `@tool name(args)` and `<tool:name>{...}` because `@` collides
//! with email addresses, and a bare XML-like tag collides with prose and with
//! `>` inside text. `<<call` is rare in ordinary answers, JSON arguments keep
//! nesting and escapes well-defined, and the closing `>>` is only special
//! outside a JSON string. The call form is one line of instruction, which is
//! easier for a model to copy than a second argument language.
//!
//! ```text
//! prompt      = "Tools\n" tool-line* hint "\n"
//! hint        = "Call <<call name {\"k\":\"v\"}>>"
//! tool-line   = name "(" fields ")" "!"? ( " " description )? "\n"
//! fields      = ( field ( "," field )* )?
//! field       = ident "?"? ":" type ( " " json-string )?
//! type        = union
//! union       = single ( "|" single )*
//! single      = atom suffix*
//! atom        = prim | format | array | object | "anyOf(" union ")" | "oneOf(" union ")"
//! prim        = "str" | "int" | "num" | "bool" | "null" | "json"
//! format      = "datetime" | "date" | "time" | "email" | "uri" | "uuid"
//! array       = "[" type "]"
//! object      = "{" fields "}"
//! suffix      = ">=" number | "<=" number | ">" number | "<" number
//!             | "#" integer? ".." integer?
//!             | "~" json-string
//!             | "!dup" | "!"
//!             | "=" enum-value ( "|" enum-value )*
//! call        = "<<call " name " " json-object ">>"
//! ```
//!
//! `?` marks an optional field. `!` after `)` or after an object type means
//! `additionalProperties: false`. `str=public|private` is a string enum.
//! `datetime` is `type: string, format: date-time`. `json` is an unconstrained
//! value. Field descriptions are JSON strings so commas and quotes stay inside
//! the field. Property order follows `serde_json`'s map order, which is sorted
//! unless the workspace enables `preserve_order`. That order is stable.
//!
//! # What is preserved
//!
//! Tool name, tool description (whitespace collapsed), required versus optional
//! fields, primitive types, arrays, nested objects, enums, `const`, numeric
//! bounds, string and array lengths, `pattern`, `uniqueItems`,
//! `additionalProperties` as a boolean, `nullable`, `anyOf` / `oneOf`, and the
//! formats `date-time`, `date`, `time`, `email`, `uri`, and `uuid`.
//!
//! # Unsupported schemas
//!
//! These bypass compaction instead of being shortened: `$ref`, `allOf`, `not`,
//! `if` / `then` / `else`, `dependentSchemas`, `dependentRequired`,
//! `patternProperties`, `prefixItems`, tuple `items`, `unevaluatedProperties`,
//! `contains`, non-boolean `additionalProperties`, non-scalar enums, unknown
//! formats, and any other keyword. A tool `type` other than `function`, a
//! duplicate name, or extension fields on the tool object also bypass.
//!
//! # Failure
//!
//! [`decode_calls`] and [`StreamDecoder`] do not guess a tool, invent a missing
//! argument, coerce `"30"` into an integer, or accept an enum value that is not
//! listed. Malformed JSON is an error even when it is almost an object.
//!
//! Call ids are `call_0`, `call_1`, … in the order calls are decoded. They are
//! deterministic. The router can replace them if it owns id assignment.
//!
//! # Example
//!
//! ```
//! use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef};
//! use serde_json::json;
//!
//! let tools = vec![ToolDef {
//!     kind: "function".into(),
//!     function: nasiko_tool_compact::FunctionDef {
//!         name: "create_calendar_event".into(),
//!         description: Some("Create a calendar event".into()),
//!         parameters: Some(json!({
//!             "type": "object",
//!             "properties": {
//!                 "title": { "type": "string" },
//!                 "start": { "type": "string", "format": "date-time" },
//!                 "duration_min": { "type": "integer" },
//!                 "visibility": { "type": "string", "enum": ["public", "private"] }
//!             },
//!             "required": ["title", "start"],
//!             "additionalProperties": false
//!         })),
//!     },
//!     extra: Default::default(),
//! }];
//! let compact = encode_tools(&tools).unwrap();
//! assert!(compact.compacted);
//! let calls = decode_calls(
//!     r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
//!     &tools,
//! ).unwrap();
//! assert_eq!(calls[0].function.name, "create_calendar_event");
//! ```

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod call;
mod cursor;
mod error;
mod prompt;
mod schema;
mod types;

pub use call::{StreamDecoder, StreamItem, decode_calls, render_call};
pub use error::Error;
pub use prompt::{CALL_HINT, decode_tools, encode_tools};
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(
        name: &str,
        description: Option<&str>,
        parameters: Option<serde_json::Value>,
    ) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: description.map(str::to_string),
                parameters,
            },
            extra: Default::default(),
        }
    }

    fn calendar() -> ToolDef {
        tool(
            "create_calendar_event",
            Some("Create a calendar event"),
            Some(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["title", "start"],
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time" },
                    "duration_min": { "type": "integer", "minimum": 1 },
                    "attendees": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "private"] },
                    "notes": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": { "body": { "type": "string" } }
                    }
                }
            })),
        )
    }

    fn args(call: &ToolCall) -> serde_json::Value {
        serde_json::from_str(&call.function.arguments).unwrap()
    }

    #[test]
    fn empty_tools_encode_to_an_empty_prompt() {
        let encoded = encode_tools(&[]).unwrap();
        assert!(encoded.compacted);
        assert!(encoded.prompt.is_empty());
        assert!(decode_tools(&encoded).unwrap().is_empty());
    }

    #[test]
    fn encodes_required_optional_enum_array_and_nested_object() {
        let encoded = encode_tools(&[calendar()]).unwrap();
        assert!(encoded.compacted);
        let signature = encoded.prompt.lines().nth(1).unwrap_or("").to_string();
        assert_eq!(
            signature,
            "create_calendar_event(attendees?:[str],duration_min?:int>=1,notes?:{body?:str}!,start:datetime,title:str \"Event title\",visibility?:str=public|private)! Create a calendar event"
        );
        assert!(encoded.prompt.contains("Create a calendar event"));
        assert!(encoded.prompt.ends_with(&format!("{CALL_HINT}\n")));
    }

    #[test]
    fn encode_is_deterministic() {
        let tools = vec![calendar(), tool("ping", None, None)];
        let a = encode_tools(&tools).unwrap();
        let b = encode_tools(&tools).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn round_trip_keeps_schema_semantics() {
        let original = calendar();
        let encoded = encode_tools(std::slice::from_ref(&original)).unwrap();
        let decoded = decode_tools(&encoded).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].function.name, "create_calendar_event");
        assert_eq!(
            decoded[0].function.description.as_deref(),
            Some("Create a calendar event")
        );
        let back = decoded[0].function.parameters.as_ref().unwrap();
        let samples = [
            json!({"title":"A","start":"2026-10-05T15:00:00Z"}),
            json!({"title":"A","start":"2026-10-05T15:00:00Z","visibility":"public","attendees":["a@b.co"]}),
            json!({"title":"A","start":"2026-10-05T15:00:00Z","duration_min":"30"}),
            json!({"start":"2026-10-05T15:00:00Z"}),
            json!({"title":"A","start":"2026-10-05T15:00:00Z","visibility":"secret"}),
            json!({"title":"A","start":"2026-10-05T15:00:00Z","extra":1}),
            json!({"title":"A","start":"not-a-date"}),
            json!({"title":"A","start":"2026-10-05T15:00:00Z","notes":{"body":"x","nope":1}}),
        ];
        for sample in samples {
            let left = schema::validate(&sample, original.function.parameters.as_ref().unwrap());
            let right = schema::validate(&sample, back);
            assert_eq!(
                left.is_ok(),
                right.is_ok(),
                "{sample} left={left:?} right={right:?}"
            );
        }
    }

    #[test]
    fn unsupported_schema_bypasses_the_whole_list() {
        let bad = tool(
            "lookup",
            None,
            Some(json!({"type":"object","properties":{"id":{"$ref":"#/$defs/Id"}}})),
        );
        let encoded = encode_tools(&[calendar(), bad]).unwrap();
        assert!(!encoded.compacted);
        assert!(encoded.prompt.is_empty());
        assert!(encoded.bypass_reason.unwrap().contains("lookup"));
    }

    #[test]
    fn decodes_one_call_and_ignores_surrounding_text() {
        let tools = vec![calendar()];
        let text = "Sure.\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nDone.";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_0");
        assert_eq!(calls[0].kind, "function");
        assert_eq!(args(&calls[0])["title"], "Design review");
        assert_eq!(args(&calls[0])["attendees"], serde_json::Value::Null);
    }

    #[test]
    fn decodes_multiple_calls_in_order() {
        let tools = vec![
            calendar(),
            tool(
                "send_mail",
                None,
                Some(json!({
                    "type":"object",
                    "required":["to"],
                    "properties":{"to":{"type":"string"},"cc":{"type":"array","items":{"type":"string"}}}
                })),
            ),
        ];
        let text = format!(
            "{}\nplain\n{}",
            render_call("send_mail", &json!({"to":"a@b.co","cc":["c@d.co"]})).unwrap(),
            render_call(
                "create_calendar_event",
                &json!({"title":"A","start":"2026-10-05T15:00:00Z"})
            )
            .unwrap()
        );
        let calls = decode_calls(&text, &tools).unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|c| c.function.name.as_str())
                .collect::<Vec<_>>(),
            vec!["send_mail", "create_calendar_event"]
        );
        assert_eq!(calls[1].id, "call_1");
    }

    #[test]
    fn plain_answer_has_no_calls() {
        let calls = decode_calls("What's the weather?", &[calendar()]).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn rejects_unknown_missing_enum_and_type() {
        let tools = vec![calendar()];
        let err = decode_calls(r#"<<call missing {"title":"A"}>>"#, &tools).unwrap_err();
        assert!(matches!(err, Error::UnknownTool(_)));

        let err = decode_calls(
            r#"<<call create_calendar_event {"duration_min":30}>>"#,
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments { .. }));

        let err = decode_calls(
            r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","visibility":"secret"}>>"#,
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments { .. }));

        let err = decode_calls(
            r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","duration_min":"30"}>>"#,
            &tools,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidArguments { .. }));
    }

    #[test]
    fn rejects_malformed_json_without_repair() {
        let err = decode_calls(
            r#"<<call create_calendar_event {"title":"A",}>>"#,
            &[calendar()],
        )
        .unwrap_err();
        assert!(matches!(err, Error::Malformed(_)));
    }

    #[test]
    fn keeps_escaped_quotes_unicode_and_marker_inside_strings() {
        let tools = vec![tool(
            "echo",
            None,
            Some(json!({
                "type":"object",
                "required":["text"],
                "properties":{"text":{"type":"string"}}
            })),
        )];
        let text = "<<call echo {\"text\":\"say \\\"hi\\\" >> café\"}>>";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(args(&calls[0])["text"], "say \"hi\" >> café");
    }

    #[test]
    fn empty_object_is_valid_when_nothing_is_required() {
        let tools = vec![tool(
            "ping",
            None,
            Some(json!({"type":"object","properties":{"n":{"type":"integer"}}})),
        )];
        let calls = decode_calls("<<call ping {}>>", &tools).unwrap();
        assert_eq!(args(&calls[0]), json!({}));
    }

    #[test]
    fn no_arg_tool_rejects_extra_fields() {
        let tools = vec![tool("ping", None, None)];
        assert!(decode_calls("<<call ping {}>>", &tools).is_ok());
        assert!(matches!(
            decode_calls(r#"<<call ping {"x":1}>>"#, &tools),
            Err(Error::InvalidArguments { .. })
        ));
    }

    #[test]
    fn nested_arguments_are_validated() {
        let tools = vec![calendar()];
        let ok = decode_calls(
            r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","notes":{"body":"x"}}>>"#,
            &tools,
        );
        assert!(ok.is_ok());
        let bad = decode_calls(
            r#"<<call create_calendar_event {"title":"A","start":"2026-10-05T15:00:00Z","notes":{"body":1}}>>"#,
            &tools,
        );
        assert!(matches!(bad, Err(Error::InvalidArguments { .. })));
    }

    #[test]
    fn every_char_split_matches_a_full_decode() {
        let tools = vec![calendar()];
        let text = format!(
            "before {}\nafter <<calling is not a call",
            render_call(
                "create_calendar_event",
                &json!({"title":"Retro","start":"2026-10-04T10:00:00+05:30","attendees":["riya@example.com"]})
            )
            .unwrap()
        );
        let expected = decode_calls(&text, &tools).unwrap();
        let mut decoder = StreamDecoder::new(&tools);
        let mut got = Vec::new();
        for ch in text.chars() {
            for item in decoder.push(&ch.to_string()).unwrap() {
                if let StreamItem::Call(call) = item {
                    got.push(call);
                }
            }
        }
        for item in decoder.finish().unwrap() {
            if let StreamItem::Call(call) = item {
                got.push(call);
            }
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn marker_prefixes_are_not_calls_until_committed() {
        let prefixes = ["<", "<<", "<<c", "<<ca", "<<cal", "<<call"];
        for prefix in prefixes {
            let mut decoder = StreamDecoder::new(&[calendar()]);
            assert!(decoder.push(prefix).unwrap().is_empty());
            let items = decoder.finish().unwrap();
            assert_eq!(items, vec![StreamItem::Text(prefix.into())]);
        }
        let mut decoder = StreamDecoder::new(&[calendar()]);
        decoder.push("<<call ").unwrap();
        let err = decoder.finish().unwrap_err();
        assert!(matches!(err, Error::Malformed(_)));
    }

    #[test]
    fn split_inside_a_string_and_before_the_closer() {
        let tools = vec![tool(
            "echo",
            None,
            Some(json!({
                "type":"object",
                "required":["text"],
                "properties":{"text":{"type":"string"}}
            })),
        )];
        let chunks = ["<<ca", "ll echo {\"text\":\"Ret", "ro >> end", "\"}>>"];
        let mut decoder = StreamDecoder::new(&tools);
        let mut calls = Vec::new();
        for chunk in chunks {
            for item in decoder.push(chunk).unwrap() {
                if let StreamItem::Call(call) = item {
                    calls.push(call);
                }
            }
        }
        calls.extend(
            decoder
                .finish()
                .unwrap()
                .into_iter()
                .filter_map(|item| match item {
                    StreamItem::Call(call) => Some(call),
                    StreamItem::Text(_) => None,
                }),
        );
        assert_eq!(calls.len(), 1);
        assert_eq!(args(&calls[0])["text"], "Retro >> end");
    }

    #[test]
    fn two_way_splits_of_a_valid_call_are_stable() {
        let tools = vec![calendar()];
        let text = render_call(
            "create_calendar_event",
            &json!({"title":"A","start":"2026-10-05T15:00:00Z"}),
        )
        .unwrap();
        let expected = decode_calls(&text, &tools).unwrap();
        for index in 0..=text.len() {
            if !text.is_char_boundary(index) {
                continue;
            }
            let mut decoder = StreamDecoder::new(&tools);
            let mut got = Vec::new();
            for item in decoder.push(&text[..index]).unwrap() {
                if let StreamItem::Call(call) = item {
                    got.push(call);
                }
            }
            for item in decoder.push(&text[index..]).unwrap() {
                if let StreamItem::Call(call) = item {
                    got.push(call);
                }
            }
            for item in decoder.finish().unwrap() {
                if let StreamItem::Call(call) = item {
                    got.push(call);
                }
            }
            assert_eq!(got, expected, "split at {index}");
        }
    }

    #[test]
    fn invalid_calls_never_yield_a_tool_call() {
        let tools = vec![calendar()];
        let bad = [
            "<<call create_calendar_event {\"title\":",
            "<<call create_calendar_event {\"visibility\":\"secret\",\"title\":\"A\",\"start\":\"2026-10-05T15:00:00Z\"}>>",
            "<<call nope {}>>",
            "<<call create_calendar_event []>>",
            ">>",
        ];
        for text in bad {
            let mut decoder = StreamDecoder::new(&tools);
            let pushed = decoder.push(text);
            let finished = pushed.and_then(|_| decoder.finish());
            if let Ok(items) = finished {
                assert!(
                    items.iter().all(|item| matches!(item, StreamItem::Text(_))),
                    "{text} produced a call"
                );
            }
        }
    }

    #[test]
    fn pattern_and_bounds_reject_bad_values() {
        let tools = vec![tool(
            "score",
            None,
            Some(json!({
                "type":"object",
                "additionalProperties": false,
                "required":["code","n"],
                "properties":{
                    "code":{"type":"string","pattern":"^[A-Z]{2}$"},
                    "n":{"type":"integer","minimum":0,"maximum":10}
                }
            })),
        )];
        assert!(decode_calls(r#"<<call score {"code":"AB","n":3}>>"#, &tools).is_ok());
        assert!(decode_calls(r#"<<call score {"code":"abc","n":3}>>"#, &tools).is_err());
        assert!(decode_calls(r#"<<call score {"code":"AB","n":11}>>"#, &tools).is_err());
        let encoded = encode_tools(&tools).unwrap();
        assert!(encoded.compacted, "{:?}", encoded.bypass_reason);
        assert!(encoded.prompt.contains("code:str~\"^[A-Z]{2}$\""));
        assert!(encoded.prompt.contains("n:int>=0<=10"));
    }
}
