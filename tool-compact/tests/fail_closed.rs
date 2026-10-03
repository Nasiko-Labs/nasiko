//! Fail-closed, escaping, Unicode and robustness tests for the decoder, through the
//! public API only.

use nasiko_tool_compact::{CompactError, StreamDecoder, ToolCall, ToolDef, decode_calls};
use serde_json::{Value, json};

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send_email".into(),
            description: Some("Send an email.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
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
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        },
    ]
}

fn decode(text: &str) -> Result<Vec<ToolCall>, CompactError> {
    decode_calls(text, &tools())
}

fn code(text: &str) -> &'static str {
    match decode(text) {
        Ok(calls) => panic!("expected an error for {text:?}, got {calls:?}"),
        Err(e) => e.code(),
    }
}

/// Decode a single `send_email` call whose `body` is the raw JSON string literal given.
fn body_of(raw_json_string: &str) -> Result<Value, CompactError> {
    let text = format!(
        r#"<<call send_email {{"to":["a@example.com"],"subject":"s","body":{raw_json_string}}}>>"#
    );
    decode(&text).map(|calls| calls[0].arguments["body"].clone())
}

// ── Fail closed ────────────────────────────────────────────────────────────────

#[test]
fn never_guesses_a_tool_name() {
    for name in [
        "send_emial",
        "Send_Email",
        "send_email2",
        "send",
        "delete_database",
    ] {
        let text =
            format!(r#"<<call {name} {{"to":["a@example.com"],"subject":"s","body":"b"}}>>"#);
        assert_eq!(code(&text), "unknown_tool", "{name}");
    }
}

#[test]
fn invalid_values_are_rejected_not_repaired() {
    let start = r#""start":"2026-10-05T15:00:00+05:30""#;
    for args in [
        format!("{{{start}}}"),                                      // missing title
        format!(r#"{{"title":7,{start}}}"#),                         // wrong type
        format!(r#"{{"title":"t",{start},"duration_min":"30"}}"#),   // numeric string
        format!(r#"{{"title":"t",{start},"duration_min":30.5}}"#),   // not an integer
        format!(r#"{{"title":"t",{start},"visibility":"secret"}}"#), // enum
        format!(r#"{{"title":"t",{start},"visibility":"Public"}}"#), // enum is case-sensitive
        format!(r#"{{"title":"t",{start},"room":"4B"}}"#),           // unknown field
        format!(r#"{{"title":"t",{start},"duration_min":null}}"#),   // null
        r#"{"title":"t","start":"tomorrow 10am"}"#.to_string(),      // not a date-time
    ] {
        let text = format!("<<call create_calendar_event {args}>>");
        assert_eq!(code(&text), "invalid_arguments", "{args}");
    }
}

#[test]
fn malformed_json_is_rejected() {
    for args in [
        r#"{"title":"t",}"#,
        r#"{'title':'t'}"#,
        r#"{"title":"t" /* note */}"#,
        r#"{"n":NaN}"#,
        r#"{title:"t"}"#,
        "{\"title\":\"line\nbreak\"}",
    ] {
        let text = format!("<<call create_calendar_event {args}>>");
        assert_eq!(code(&text), "invalid_syntax", "{args}");
    }
}

#[test]
fn malformed_markers_are_rejected() {
    for text in [
        "<<call send_email>>",
        "<<call send_email [\"a\"]>>",
        "<<call  {}>>",
        "<<call send_email {} > >",
        "<<call send_email {}>]",
        "<<call send-email! {}>>",
    ] {
        assert_eq!(code(text), "invalid_syntax", "{text}");
    }
    for text in [
        "<<call send_email {\"to\":[",
        "<<call send_email {}>",
        "<<call ",
    ] {
        assert_eq!(code(text), "incomplete_stream", "{text}");
    }
}

#[test]
fn one_bad_call_rejects_every_call() {
    let good = r#"<<call send_email {"to":["a@example.com"],"subject":"s","body":"b"}>>"#;
    let bad = r#"<<call create_calendar_event {"title":"t"}>>"#;
    assert!(decode(&format!("{good} {bad}")).is_err());
    assert!(decode(&format!("{bad} {good}")).is_err());
}

#[test]
fn text_that_only_looks_like_markup_is_not_a_call() {
    for text in [
        "Use <<call>> syntax",
        "a << b >> c",
        "<<caller {}>>",
        "```\n<<call_me {}>>\n```",
    ] {
        assert_eq!(decode(text).unwrap(), vec![], "{text}");
    }
}

// ── Escaping ───────────────────────────────────────────────────────────────────

#[test]
fn close_marker_and_braces_inside_strings() {
    assert_eq!(body_of(r#""a >> b""#).unwrap(), json!("a >> b"));
    assert_eq!(body_of(r#""}>> {""#).unwrap(), json!("}>> {"));
    assert_eq!(
        body_of(r#""<<call send_email {}>>""#).unwrap(),
        json!("<<call send_email {}>>")
    );
}

#[test]
fn escape_sequences() {
    assert_eq!(body_of(r#""say \"hi\"""#).unwrap(), json!("say \"hi\""));
    assert_eq!(body_of(r#""C:\\temp\\""#).unwrap(), json!("C:\\temp\\"));
    assert_eq!(body_of(r#""\\\" >> ""#).unwrap(), json!("\\\" >> "));
    assert_eq!(
        body_of(r#""line1\nline2\ttab""#).unwrap(),
        json!("line1\nline2\ttab")
    );
    assert_eq!(
        body_of(r#""{\"nested\":[1,{\"x\":\">>\"}]}""#).unwrap(),
        json!("{\"nested\":[1,{\"x\":\">>\"}]}")
    );
}

#[test]
fn unicode() {
    assert_eq!(body_of(r#""नमस्ते 😀 café""#).unwrap(), json!("नमस्ते 😀 café"));
    assert_eq!(
        body_of(r#""caf\u00e9 \ud83d\ude00""#).unwrap(),
        json!("café 😀")
    );
    assert!(
        body_of(r#""\ud800""#).is_err(),
        "lone surrogate must be rejected"
    );
    let calls = decode("मीटिंग बुक करें 📅 <<call create_calendar_event {\"title\":\"समीक्षा\",\"start\":\"2026-10-05T15:00:00+05:30\"}>> ✅").unwrap();
    assert_eq!(calls[0].arguments["title"], json!("समीक्षा"));
}

// ── Robustness: large and hostile input ───────────────────────────────────────

#[test]
fn many_calls() {
    let call = r#"<<call send_email {"to":["a@example.com"],"subject":"s","body":"b"}>>"#;
    let text = vec![call; 10_000].join("\n");
    assert_eq!(decode(&text).unwrap().len(), 10_000);
}

#[test]
fn long_plain_text_and_marker_noise() {
    assert_eq!(decode(&"lorem ipsum ".repeat(400_000)).unwrap(), vec![]);
    assert_eq!(decode(&"<".repeat(1_000_000)).unwrap(), vec![]);
    assert_eq!(decode(&"<<ca".repeat(250_000)).unwrap(), vec![]);
}

#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let deep_arrays = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let text = format!(r#"<<call send_email {{"to":{deep_arrays}}}>>"#);
    assert!(decode(&text).is_err());

    let deep_objects = format!("{}{}", "{\"a\":".repeat(10_000), "}".repeat(10_000));
    let text = format!("<<call send_email {{\"to\":{deep_objects}}}>>");
    assert!(decode(&text).is_err());
}

#[test]
fn oversized_or_unterminated_arguments() {
    let huge = format!(
        r#"<<call send_email {{"to":["a@example.com"],"subject":"s","body":"{}"}}>>"#,
        "x".repeat(2 << 20)
    );
    assert_eq!(code(&huge), "invalid_syntax");
    let unterminated = format!(r#"<<call send_email {{"body":"{}"#, "x".repeat(100_000));
    assert_eq!(code(&unterminated), "incomplete_stream");
}

#[test]
fn stream_one_char_at_a_time_through_a_hostile_mix() {
    let text = "a<<b<<<call send_email {\"to\":[\"x@example.com\"],\"subject\":\"}>> \\\" é😀\",\"body\":\"\"}>>z";
    let mut decoder = StreamDecoder::new(&tools());
    for (i, _) in text.char_indices() {
        let next = text[i..].chars().next().unwrap();
        decoder.push(&text[i..i + next.len_utf8()]).unwrap();
    }
    let calls = decoder.finish().unwrap();
    assert_eq!(calls, decode(text).unwrap());
    assert_eq!(calls[0].arguments["subject"], json!("}>> \" é😀"));
}

#[test]
fn first_error_in_text_order_wins_regardless_of_chunking() {
    // A semantic error (unknown tool) followed by a later syntax error: the reported
    // error must not depend on where the chunk boundaries fall.
    let text = "<<call nope {}>> then <<call !";
    let whole = decode(text);
    assert_eq!(whole, Err(CompactError::UnknownTool("nope".into())));
    for (i, _) in text.char_indices().skip(1) {
        let mut decoder = StreamDecoder::new(&tools());
        let split = decoder
            .push(&text[..i])
            .and_then(|_| decoder.push(&text[i..]))
            .and_then(|_| decoder.finish());
        assert_eq!(split, whole, "split at {i}");
    }
}
