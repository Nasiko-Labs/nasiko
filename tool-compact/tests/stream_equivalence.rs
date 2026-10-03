//! Property tests for the decoder: streamed decoding equals whole-string decoding for
//! any input and any chunking, valid calls round-trip exactly, and no input ever
//! yields a call that is not a known tool with schema-valid arguments.

use nasiko_tool_compact::{
    StreamDecoder, StreamEvent, ToolCall, ToolDef, decode_calls, render_call, validate_arguments,
};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send_email".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                    "cc": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["to", "subject", "body"]
            })),
        },
        ToolDef {
            name: "create_calendar_event".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "private": {"type": "boolean"},
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

/// Everything observable about a decode: the passed-through text (only meaningful on
/// success) and the final result.
#[derive(Debug, PartialEq)]
struct Outcome {
    text: String,
    result: Result<Vec<ToolCall>, nasiko_tool_compact::CompactError>,
}

fn stream(chunks: &[&str]) -> Outcome {
    let mut decoder = StreamDecoder::new(&tools());
    let mut text = String::new();
    for chunk in chunks {
        match decoder.push(chunk) {
            Ok(events) => {
                for event in events {
                    if let StreamEvent::Text(t) = event {
                        text.push_str(&t);
                    }
                }
            }
            Err(e) => {
                return Outcome {
                    text: String::new(),
                    result: Err(e),
                };
            }
        }
    }
    let result = decoder.finish();
    if result.is_err() {
        text.clear();
    }
    Outcome { text, result }
}

/// Split `s` at the given character positions (deduplicated, clamped).
fn split_at_chars(s: &str, mut cuts: Vec<usize>) -> Vec<&str> {
    let boundaries: Vec<usize> = s.char_indices().map(|(i, _)| i).skip(1).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut pieces = Vec::new();
    let mut last = 0;
    for cut in cuts {
        if let Some(&at) = boundaries.get(cut % boundaries.len().max(1))
            && at > last
        {
            pieces.push(&s[last..at]);
            last = at;
        }
    }
    pieces.push(&s[last..]);
    pieces
}

/// Fragments chosen to drive the scanner through every state, including the awkward
/// ones: partial markers, quotes, escapes, braces, `>>`, multi-byte characters.
fn arb_hostile_text() -> impl Strategy<Value = String> {
    let fragments = prop_oneof![
        Just("<"),
        Just("<<"),
        Just("<<call "),
        Just("<<call"),
        Just("call "),
        Just("send_email "),
        Just("get_time"),
        Just("create_calendar_event"),
        Just(" "),
        Just("\n"),
        Just("{"),
        Just("}"),
        Just("{}"),
        Just("\""),
        Just("\\"),
        Just("\\\""),
        Just(">"),
        Just(">>"),
        Just(":"),
        Just(","),
        Just("["),
        Just("]"),
        Just("\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\""),
        Just("x"),
        Just("é"),
        Just("😀"),
        Just("नम"),
    ];
    prop::collection::vec(fragments, 0..40).prop_map(|parts| parts.concat())
}

/// A string argument that stresses escaping: quotes, backslashes, markers, Unicode.
fn arb_tricky_string() -> impl Strategy<Value = String> {
    prop_oneof![
        "\\PC{0,24}",
        Just(String::new()),
        Just("a >> b".to_string()),
        Just("}>> {\"".to_string()),
        Just("\\\" <<call get_time {}>>".to_string()),
        Just("line\nbreak\ttab".to_string()),
        Just("नमस्ते 😀".to_string()),
    ]
}

fn arb_email() -> impl Strategy<Value = ToolCall> {
    (
        prop::collection::vec(arb_tricky_string(), 0..3),
        arb_tricky_string(),
        arb_tricky_string(),
        prop::option::of(prop::collection::vec(arb_tricky_string(), 0..2)),
    )
        .prop_map(|(to, subject, body, cc)| {
            let mut args = Map::new();
            args.insert("to".into(), json!(to));
            args.insert("subject".into(), json!(subject));
            args.insert("body".into(), json!(body));
            if let Some(cc) = cc {
                args.insert("cc".into(), json!(cc));
            }
            ToolCall {
                name: "send_email".into(),
                arguments: Value::Object(args),
            }
        })
}

fn arb_event() -> impl Strategy<Value = ToolCall> {
    (
        arb_tricky_string(),
        prop_oneof![
            Just("2026-10-05T15:00:00+05:30"),
            Just("2026-10-04T10:00:00Z"),
            Just("2026-12-31T23:59:59.5-08:00"),
        ],
        prop::option::of(any::<i64>()),
        prop::option::of(any::<bool>()),
        prop::option::of(prop_oneof![Just("public"), Just("private")]),
    )
        .prop_map(|(title, start, duration, private, visibility)| {
            let mut args = Map::new();
            args.insert("title".into(), json!(title));
            args.insert("start".into(), json!(start));
            if let Some(d) = duration {
                args.insert("duration_min".into(), json!(d));
            }
            if let Some(p) = private {
                args.insert("private".into(), json!(p));
            }
            if let Some(v) = visibility {
                args.insert("visibility".into(), json!(v));
            }
            ToolCall {
                name: "create_calendar_event".into(),
                arguments: Value::Object(args),
            }
        })
}

fn arb_call() -> impl Strategy<Value = ToolCall> {
    prop_oneof![
        arb_email(),
        arb_event(),
        Just(ToolCall {
            name: "get_time".into(),
            arguments: json!({}),
        }),
    ]
}

/// Text around calls that cannot itself open a call.
fn arb_plain_text() -> impl Strategy<Value = String> {
    "[^<]{0,20}"
}

/// Valid model output: calls interleaved with plain text.
fn arb_valid_output() -> impl Strategy<Value = (String, Vec<ToolCall>)> {
    (
        arb_plain_text(),
        prop::collection::vec((arb_call(), arb_plain_text()), 0..4),
    )
        .prop_map(|(lead, rest)| {
            let mut text = lead;
            let mut calls = Vec::new();
            for (call, trailing) in rest {
                text.push_str(&render_call(&call));
                text.push_str(&trailing);
                calls.push(call);
            }
            (text, calls)
        })
}

/// Independent oracle: every returned call names a known tool and passes validation.
fn assert_all_valid(calls: &[ToolCall]) {
    let tools = tools();
    for call in calls {
        let tool = tools
            .iter()
            .find(|t| t.name == call.name)
            .unwrap_or_else(|| panic!("decoder produced unknown tool {:?}", call.name));
        match &tool.parameters {
            Some(schema) => validate_arguments(schema, &call.arguments).unwrap(),
            None => assert_eq!(call.arguments, json!({})),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any input, any chunking: identical calls, identical error, identical text.
    #[test]
    fn random_chunking_matches_whole_decode(
        text in arb_hostile_text(),
        cuts in prop::collection::vec(any::<usize>(), 0..12),
    ) {
        let whole = stream(&[&text]);
        prop_assert_eq!(&whole.result, &decode_calls(&text, &tools()));
        prop_assert_eq!(stream(&split_at_chars(&text, cuts)), whole);
    }

    /// Every single split point, and one character at a time.
    #[test]
    fn every_split_point_matches_whole_decode(text in arb_hostile_text()) {
        let whole = stream(&[&text]);
        for (i, _) in text.char_indices().skip(1) {
            prop_assert_eq!(stream(&[&text[..i], &text[i..]]), stream(&[&text]));
        }
        let chars: Vec<String> = text.chars().map(String::from).collect();
        let chars: Vec<&str> = chars.iter().map(String::as_str).collect();
        prop_assert_eq!(stream(&chars), whole);
    }

    /// Rendered valid calls decode back to exactly the same calls, however split.
    #[test]
    fn valid_calls_round_trip(
        (text, calls) in arb_valid_output(),
        cuts in prop::collection::vec(any::<usize>(), 0..12),
    ) {
        prop_assert_eq!(decode_calls(&text, &tools()).unwrap(), calls.clone());
        prop_assert_eq!(stream(&split_at_chars(&text, cuts)).result.unwrap(), calls);
    }

    /// Arbitrary input never panics and never yields an unknown tool or invalid arguments.
    #[test]
    fn arbitrary_input_never_yields_an_invalid_call(text in "\\PC{0,200}") {
        if let Ok(calls) = decode_calls(&text, &tools()) {
            assert_all_valid(&calls);
        }
    }

    /// Damaging a valid output by one character either fails or still yields only
    /// valid calls: a corrupted call is never silently "repaired".
    #[test]
    fn corrupted_output_is_rejected_or_still_valid(
        (text, _) in arb_valid_output(),
        at in any::<usize>(),
    ) {
        let positions: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        if let Some(&i) = positions.get(at % positions.len().max(1)) {
            let mut damaged = text.clone();
            damaged.remove(i);
            if let Ok(calls) = decode_calls(&damaged, &tools()) {
                assert_all_valid(&calls);
            }
        }
    }
}
