//! `decode_calls` / `StreamDecoder` behaviour on whole replies.

mod common;

use common::tool;
use nasiko_tool_compact::{
    StreamDecoder, ToolCompactError, ToolDef, decode_calls, encode_call, limits, validate_call,
    validate_calls,
};
use serde_json::{Value, json};

fn catalog() -> Vec<ToolDef> {
    vec![
        tool(
            "create_calendar_event",
            json!({"type": "object", "properties": {
                "title": {"type": "string"},
                "start": {"type": "string", "format": "date-time"},
                "duration_min": {"type": "integer"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "meta": {"type": "object", "properties": {"k": {"type": "string"}}}
            }, "required": ["title", "start"]}),
        ),
        tool(
            "send_email",
            json!({"type": "object", "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            }, "required": ["to", "subject", "body"]}),
        ),
        tool("noop", json!({"type": "object", "properties": {}})),
    ]
}

fn ok(text: &str) -> (String, Vec<(String, Value)>) {
    let d = decode_calls(text, &catalog()).unwrap_or_else(|e| panic!("{text:?}: {e}"));
    (
        d.content,
        d.calls.into_iter().map(|c| (c.name, c.arguments)).collect(),
    )
}

fn err(text: &str) -> ToolCompactError {
    match decode_calls(text, &catalog()) {
        Err(e) => e,
        Ok(d) => panic!("{text:?} should fail, got {d:?}"),
    }
}

#[test]
fn no_call_is_plain_content() {
    let (content, calls) = ok("The weather is fine. No tool needed.");
    assert_eq!(content, "The weather is fine. No tool needed.");
    assert!(calls.is_empty());
    let (content, calls) = ok("");
    assert_eq!(content, "");
    assert!(calls.is_empty());
}

#[test]
fn one_call_with_and_without_prose() {
    let (content, calls) = ok(
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
    );
    assert_eq!(content, "");
    assert_eq!(
        calls,
        vec![(
            "create_calendar_event".to_owned(),
            json!({"title": "Design review", "start": "2026-10-05T15:00:00+05:30"})
        )]
    );

    let (content, calls) = ok(
        "Sure, booking it now.\n<<call create_calendar_event {\"title\":\"x\",\"start\":\"s\"}>>\nDone.",
    );
    assert_eq!(content, "Sure, booking it now.\n\nDone.");
    assert_eq!(calls.len(), 1);
}

#[test]
fn multiple_calls_in_order_with_prose_between() {
    let text = "First:\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build status\",\"body\":\"green\"}>>\nthen\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\",\"duration_min\":30,\"visibility\":\"private\"}>>";
    let (content, calls) = ok(text);
    assert_eq!(content, "First:\n\nthen\n");
    assert_eq!(calls[0].0, "send_email");
    assert_eq!(calls[1].0, "create_calendar_event");
    assert_eq!(calls[1].1["duration_min"], json!(30));
}

#[test]
fn double_angle_not_followed_by_call_is_text() {
    for text in [
        "a << b",
        "<<",
        "<",
        "<<caller x",
        "<< call noop {}>>",
        "<<cal",
        "x <<<< y",
        "<<callx",
    ] {
        let (content, calls) = ok(text);
        assert_eq!(content, text, "{text:?}");
        assert!(calls.is_empty(), "{text:?}");
    }
    // A false start right before a real marker still finds the call.
    let (content, calls) = ok("<<<call noop {}>>");
    assert_eq!(content, "<");
    assert_eq!(calls.len(), 1);
    let (content, calls) = ok("<<ca<<call noop {}>>");
    assert_eq!(content, "<<ca");
    assert_eq!(calls.len(), 1);
}

#[test]
fn closing_markers_and_braces_inside_strings_do_not_end_the_call() {
    let (_, calls) =
        ok(r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#);
    assert_eq!(calls[0].1["subject"], json!("a >> b"));
    let (_, calls) =
        ok(r#"<<call send_email {"to":["s"],"subject":"}>>{","body":"\"quoted\" \\ back"}>>"#);
    assert_eq!(calls[0].1["subject"], json!("}>>{"));
    assert_eq!(calls[0].1["body"], json!("\"quoted\" \\ back"));
    let (_, calls) =
        ok("<<call send_email {\"to\":[\"s\"],\"subject\":\"日本語 🎉\",\"body\":\"\\u00e9\"}>>");
    assert_eq!(calls[0].1["subject"], json!("日本語 🎉"));
    assert_eq!(calls[0].1["body"], json!("é"));
}

#[test]
fn nested_json_and_whitespace_variants_are_accepted() {
    let (_, calls) = ok(
        "<<call create_calendar_event\n{\"title\":\"t\",\"start\":\"s\",\"meta\":{\"k\":\"v\"},\"attendees\":[\"a\",\"b\"]}\n>>",
    );
    assert_eq!(calls[0].1["meta"], json!({"k": "v"}));
    let (_, calls) = ok("<<call   noop   {  }  >>");
    assert_eq!(calls[0].0, "noop");
    let (_, calls) = ok("<<call noop{}>>");
    assert_eq!(calls[0].0, "noop");
}

#[test]
fn unknown_tools_missing_fields_wrong_types_and_bad_enums_fail() {
    assert_eq!(err("<<call delete_everything {}>>").kind(), "unknown_tool");
    assert_eq!(
        err("<<call create_calendar_event {\"start\":\"s\",\"visibility\":\"secret\"}>>").kind(),
        "invalid_arguments"
    );
    assert_eq!(err("<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"visibility\":\"secret\"}>>").kind(), "invalid_arguments");
    assert_eq!(err("<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"duration_min\":\"30\"}>>").kind(), "invalid_arguments");
    assert_eq!(
        err(
            "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"duration_min\":30.5}>>"
        )
        .kind(),
        "invalid_arguments"
    );
    assert_eq!(
        err(
            "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"attendees\":\"a@b\"}>>"
        )
        .kind(),
        "invalid_arguments"
    );
}

#[test]
fn malformed_json_duplicate_keys_and_bad_framing_fail() {
    assert_eq!(
        err("<<call noop {\"a\":1,\"a\":2}>>").kind(),
        "malformed_call"
    );
    assert_eq!(err("<<call noop {not json}>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop {\"a\":}>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop {}>").kind(), "incomplete_call");
    assert_eq!(err("<<call noop {}").kind(), "incomplete_call");
    assert_eq!(
        err("<<call noop {\"a\":\"unterminated").kind(),
        "incomplete_call"
    );
    assert_eq!(err("<<call noop").kind(), "incomplete_call");
    assert_eq!(err("<<call ").kind(), "incomplete_call");
    assert_eq!(err("<<call noop {} x>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop {}>x").kind(), "malformed_call");
    assert_eq!(err("<<call no$op {}>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop []>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop (x)>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop ({})>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop ()x>>").kind(), "malformed_call");
    assert_eq!(err("<<call noop (").kind(), "incomplete_call");
    assert_eq!(err("<<call noop ()").kind(), "incomplete_call");
    assert_eq!(err("<<call noop>").kind(), "incomplete_call");
    // A bare marker at the very end never saw the whitespace that starts a call: it stays prose.
    // One more character of whitespace and it is an open call.
    assert_eq!(ok("see <<call").0, "see <<call");
    assert_eq!(err("see <<call ").kind(), "incomplete_call");
}

#[test]
fn a_valid_call_followed_by_an_invalid_one_releases_nothing() {
    let text = "<<call noop {}>>\n<<call create_calendar_event {\"title\":\"t\"}>>";
    let e = err(text);
    assert_eq!(e.kind(), "invalid_arguments");
    // Through the streaming API the first call was validated, but finish still returns the error.
    let mut d = StreamDecoder::new(&catalog()).unwrap();
    d.push("<<call noop {}>>\n").unwrap();
    assert!(d.push("<<call unknown_tool {}>>").is_err());
    assert_eq!(d.finish().unwrap_err().kind(), "unknown_tool");
}

#[test]
fn errors_never_echo_argument_values() {
    let e = err(
        "<<call create_calendar_event {\"title\":\"SECRET-VALUE\",\"start\":\"s\",\"visibility\":\"ALSO-SECRET\"}>>",
    );
    assert!(!e.to_string().contains("SECRET"));
    let e = err("<<call noop {\"k\":\"SECRET-VALUE\",\"k\":1}>>");
    assert!(!e.to_string().contains("SECRET"));
}

#[test]
fn limits_are_enforced_at_exact_boundaries() {
    let cat = catalog();
    let one = "<<call noop {}>>";
    assert!(decode_calls(&one.repeat(limits::MAX_CALLS), &cat).is_ok());
    assert!(matches!(
        decode_calls(&one.repeat(limits::MAX_CALLS + 1), &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_CALLS",
            ..
        })
    ));

    let long_name = "n".repeat(limits::MAX_NAME_LEN + 1);
    assert!(matches!(
        decode_calls(&format!("<<call {long_name} {{}}>>"), &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_NAME_LEN",
            ..
        })
    ));
    let exact_name = "n".repeat(limits::MAX_NAME_LEN);
    assert_eq!(
        decode_calls(&format!("<<call {exact_name} {{}}>>"), &cat)
            .unwrap_err()
            .kind(),
        "unknown_tool"
    );

    // Per-call argument bytes: the JSON object including braces must fit.
    let body = |n: usize| {
        format!(
            "<<call send_email {{\"to\":[\"s\"],\"subject\":\"x\",\"body\":\"{}\"}}>>",
            "b".repeat(n)
        )
    };
    let overhead = body(0).len() - "<<call send_email >>".len();
    assert!(decode_calls(&body(limits::MAX_ARGS_BYTES - overhead), &cat).is_ok());
    assert!(matches!(
        decode_calls(&body(limits::MAX_ARGS_BYTES - overhead + 1), &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_ARGS_BYTES",
            ..
        })
    ));

    // Aggregate argument bytes across calls.
    let big = body(200 * 1024);
    assert!(decode_calls(&format!("{big}{big}"), &cat).is_ok());
    assert!(matches!(
        decode_calls(&format!("{big}{big}{big}"), &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_TOTAL_ARGS_BYTES",
            ..
        })
    ));

    // Depth inside the arguments.
    let deep = |d: usize| format!("<<call noop {{\"a\":{}{}}}>>", "[".repeat(d), "]".repeat(d));
    assert!(decode_calls(&deep(limits::MAX_DEPTH - 1), &cat).is_ok());
    assert!(matches!(
        decode_calls(&deep(limits::MAX_DEPTH), &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_DEPTH",
            ..
        })
    ));

    // Total bytes pushed, prose included.
    let prose = "p".repeat(limits::MAX_RESPONSE_BYTES);
    assert!(decode_calls(&prose, &cat).is_ok());
    let mut d = StreamDecoder::new(&cat).unwrap();
    d.push(&prose).unwrap();
    assert!(matches!(
        d.push("p"),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_RESPONSE_BYTES",
            ..
        })
    ));
}

#[test]
fn validate_call_mirrors_the_decoder_for_native_calls() {
    let cat = catalog();
    let call = validate_call(
        "send_email",
        r#"{"to":["a"],"subject":"s","body":"b"}"#,
        &cat,
    )
    .unwrap();
    assert_eq!(call.name, "send_email");
    assert_eq!(
        validate_call("nope", "{}", &cat).unwrap_err().kind(),
        "unknown_tool"
    );
    assert_eq!(
        validate_call("send_email", r#"{"to":["a"]}"#, &cat)
            .unwrap_err()
            .kind(),
        "invalid_arguments"
    );
    assert_eq!(
        validate_call("noop", r#"{"a":1,"a":2}"#, &cat)
            .unwrap_err()
            .kind(),
        "malformed_call"
    );
    assert_eq!(
        validate_call("noop", "not json", &cat).unwrap_err().kind(),
        "malformed_call"
    );
    assert_eq!(
        validate_call("noop", "[]", &cat).unwrap_err().kind(),
        "malformed_call"
    );
    assert_eq!(
        validate_call("noop", "{} {}", &cat).unwrap_err().kind(),
        "malformed_call"
    );
}

#[test]
fn an_unsupported_schema_never_yields_decoded_calls_even_when_arguments_violate_it() {
    // A `pattern` constraint the crate cannot check, with arguments that violate it. Every path
    // must refuse, not fall back to name-only decoding.
    let tools = vec![tool(
        "set_code",
        json!({"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]{3}$"}}, "required": ["code"]}),
    )];
    let text = r#"<<call set_code {"code":"not-three-caps"}>>"#;
    assert_eq!(
        decode_calls(text, &tools).unwrap_err().kind(),
        "unsupported_schema"
    );
    assert_eq!(
        StreamDecoder::new(&tools).err().map(|e| e.kind()),
        Some("unsupported_schema")
    );
    assert_eq!(
        validate_call("set_code", r#"{"code":"not-three-caps"}"#, &tools)
            .unwrap_err()
            .kind(),
        "unsupported_schema"
    );
    // Even arguments that would satisfy the pattern are not released: the catalog is unusable.
    assert_eq!(
        validate_call("set_code", r#"{"code":"ABC"}"#, &tools)
            .unwrap_err()
            .kind(),
        "unsupported_schema"
    );
}

#[test]
fn encode_call_renders_what_the_decoder_accepts() {
    let args = json!({"to": ["sam@example.com"], "subject": "a >> b", "body": "x\"y"});
    let text = encode_call("send_email", &args);
    assert_eq!(
        text,
        r#"<<call send_email {"body":"x\"y","subject":"a >> b","to":["sam@example.com"]}>>"#
    );
    let d = decode_calls(&text, &catalog()).unwrap();
    assert_eq!(d.calls[0].arguments, args);
    assert_eq!(encode_call("noop", &json!({})), "<<call noop {}>>");
}

#[test]
fn native_calls_obey_the_same_limits_as_the_decoder() {
    let cat = catalog();
    let ok = r#"{"to":["s"],"subject":"x","body":"b"}"#;
    // Per-call argument bytes, checked before parsing: an oversized blob of invalid JSON still
    // reports the limit, never a parse error.
    let big = format!(
        "{{\"to\":[\"s\"],\"subject\":\"x\",\"body\":\"{}\"}}",
        "b".repeat(limits::MAX_ARGS_BYTES)
    );
    assert!(matches!(
        validate_call("send_email", &big, &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_ARGS_BYTES",
            ..
        })
    ));
    let garbage = "x".repeat(limits::MAX_ARGS_BYTES + 1);
    assert!(matches!(
        validate_call("send_email", &garbage, &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_ARGS_BYTES",
            ..
        })
    ));
    let exact = format!(
        "{{\"to\":[\"s\"],\"subject\":\"x\",\"body\":\"{}\"}}",
        "b".repeat(limits::MAX_ARGS_BYTES - ok.len() + 1)
    );
    assert_eq!(exact.len(), limits::MAX_ARGS_BYTES);
    assert!(validate_call("send_email", &exact, &cat).is_ok());

    // Call count across the batch.
    let many: Vec<(&str, &str)> = vec![("noop", "{}"); limits::MAX_CALLS];
    assert_eq!(
        validate_calls(&many, &cat).unwrap().len(),
        limits::MAX_CALLS
    );
    let too_many: Vec<(&str, &str)> = vec![("noop", "{}"); limits::MAX_CALLS + 1];
    assert!(matches!(
        validate_calls(&too_many, &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_CALLS",
            ..
        })
    ));

    // Aggregate argument bytes across the batch, checked before any call is parsed.
    let chunk = format!(
        "{{\"to\":[\"s\"],\"subject\":\"x\",\"body\":\"{}\"}}",
        "b".repeat(200 * 1024)
    );
    let two: Vec<(&str, &str)> = vec![("send_email", &chunk), ("send_email", &chunk)];
    assert_eq!(validate_calls(&two, &cat).unwrap().len(), 2);
    let three: Vec<(&str, &str)> = vec![("send_email", &chunk); 3];
    assert!(matches!(
        validate_calls(&three, &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_TOTAL_ARGS_BYTES",
            ..
        })
    ));

    // A valid call followed by an oversized or invalid one releases nothing.
    let valid_then_oversized: Vec<(&str, &str)> = vec![("noop", "{}"), ("send_email", &big)];
    assert!(matches!(
        validate_calls(&valid_then_oversized, &cat),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_ARGS_BYTES",
            ..
        })
    ));
    let valid_then_invalid: Vec<(&str, &str)> = vec![("noop", "{}"), ("send_email", "{}")];
    assert_eq!(
        validate_calls(&valid_then_invalid, &cat)
            .unwrap_err()
            .kind(),
        "invalid_arguments"
    );
    let valid_then_unknown: Vec<(&str, &str)> = vec![("noop", "{}"), ("nope", "{}")];
    assert_eq!(
        validate_calls(&valid_then_unknown, &cat)
            .unwrap_err()
            .kind(),
        "unknown_tool"
    );
    // Unsupported catalogs refuse batches too.
    let unsupported = vec![tool(
        "set_code",
        json!({"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]{3}$"}}}),
    )];
    assert_eq!(
        validate_calls(&[("set_code", r#"{"code":"ABC"}"#)], &unsupported)
            .unwrap_err()
            .kind(),
        "unsupported_schema"
    );
}

#[test]
fn a_tool_without_arguments_may_be_called_with_empty_parentheses_or_nothing() {
    let expected = vec![("noop".to_owned(), json!({}))];
    for text in [
        "<<call noop {}>>",
        "<<call noop>>",
        "<<call noop >>",
        "<<call noop()>>",
        "<<call noop ()>>",
        "<<call noop ( ) >>",
        "<<call noop(\n)>>",
    ] {
        let (content, calls) = ok(text);
        assert_eq!(calls, expected, "{text:?}");
        assert!(content.is_empty(), "{text:?}");
    }
    let (content, calls) = ok("Checking.\n<<call noop()>>\nDone.");
    assert_eq!(content, "Checking.\n\nDone.");
    assert_eq!(calls, expected);
    // The spelling changes nothing about validation: the empty object still has to satisfy the
    // schema, so a tool with required arguments rejects it exactly like `{}`.
    for text in [
        "<<call create_calendar_event>>",
        "<<call create_calendar_event()>>",
    ] {
        assert_eq!(err(text).kind(), "invalid_arguments", "{text:?}");
    }
    assert_eq!(err("<<call missing()>>").kind(), "unknown_tool");
    assert_eq!(err("<<call missing>>").kind(), "unknown_tool");
}
