//! Model reply → validated calls. Mirrors the public eval's decoder cases, plus the fail-closed
//! edges around them.

mod common;

use common::{nested_tool, sample_tools};
use nasiko_tool_compact::{
    Decoded, StreamDecoder, ToolCall, decode_calls, encode_tools, render_call,
};
use serde_json::{Value, json};

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        name: name.into(),
        arguments: args.as_object().unwrap().clone(),
    }
}

fn calls(text: &str) -> Vec<ToolCall> {
    decode_calls(text, &sample_tools()).unwrap().calls
}

fn err_code(text: &str) -> &'static str {
    decode_calls(text, &sample_tools()).unwrap_err().code()
}

fn stream(chunks: &[&str]) -> nasiko_tool_compact::Result<Decoded> {
    let mut d = StreamDecoder::new(&sample_tools());
    for c in chunks {
        d.push(c)?;
    }
    d.finish()
}

const SEND: &str = r#"{"to":["sam@example.com"],"subject":"s","body":"b"}"#;

#[test]
fn valid_single_call() {
    let got = calls(
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
    );
    assert_eq!(
        got,
        vec![call(
            "create_calendar_event",
            json!({"title":"Design review","start":"2026-10-05T15:00:00+05:30"})
        )]
    );
    // `arguments_json` is the OpenAI `function.arguments` string and parses back to the same map.
    let reparsed: Value = serde_json::from_str(&got[0].arguments_json()).unwrap();
    assert_eq!(reparsed, Value::Object(got[0].arguments.clone()));
}

#[test]
fn multiple_calls_with_text_before_between_and_after() {
    let text = concat!(
        "Sure, doing both.\n",
        r#"<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>>"#,
        "\nand then\n",
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>"#,
        "\nDone."
    );
    let got = decode_calls(text, &sample_tools()).unwrap();
    assert_eq!(got.calls.len(), 2);
    assert_eq!(got.calls[0].name, "send_email");
    assert_eq!(got.calls[1].name, "create_calendar_event");
    assert_eq!(got.calls[1].arguments["duration_min"], json!(30));
    assert_eq!(got.text, "Sure, doing both.\n\nand then\n\nDone.");
}

#[test]
fn text_keeps_every_byte_except_the_markers() {
    let text = format!("  Prose  <<\u{3000}call send_email {SEND}>>\t tail \n");
    let got = decode_calls(&text, &sample_tools()).unwrap();
    assert_eq!(got.calls.len(), 1);
    assert_eq!(got.text, "  Prose  \t tail \n", "no trimming, no joining");
}

#[test]
fn plain_text_has_no_calls_and_is_returned_unchanged() {
    for text in [
        "I can't check the weather.",
        "",
        "use a <<callback>> and a << shift",
        "<<callback>> glued words without a brace are prose",
        "<<<< <<calendar {}>>",
    ] {
        let got = decode_calls(text, &sample_tools()).unwrap();
        assert_eq!(got.calls, vec![], "{text}");
        assert_eq!(got.text, text, "{text}");
    }
}

#[test]
fn marker_rule() {
    // Whitespace after "<<" and any letter case for "call".
    assert_eq!(calls(&format!("<< call send_email {SEND}>>")).len(), 1);
    assert_eq!(calls(&format!("<<CALL send_email {SEND}>>")).len(), 1);
    assert_eq!(calls(&format!("<<Call send_email {SEND}>>")).len(), 1);
    // A tripled "<" still finds the call, and keeps the extra "<" as text.
    let got = decode_calls(&format!("<<<call send_email {SEND}>>"), &sample_tools()).unwrap();
    assert_eq!((got.calls.len(), got.text.as_str()), (1, "<"));

    assert_eq!(err_code(r#"<< call x {"a":1}>>"#), "unknown_tool");
    assert_eq!(
        err_code(&format!("<<callsend_email {SEND}>>")),
        "invalid_arguments"
    );
    assert_eq!(err_code("<<call NAME {json args}>>"), "unknown_tool");
    assert_eq!(err_code(r#"<<call NAME {"x":1}>>"#), "unknown_tool");
}

#[test]
fn tool_names_match_exactly() {
    let mut tools = sample_tools();
    let mut longer = tools[1].clone();
    longer.name = "send_email2".into();
    tools.push(longer);
    let got = decode_calls(&format!("<<call send_email2 {SEND}>>"), &tools).unwrap();
    assert_eq!(got.calls[0].name, "send_email2");
    let got = decode_calls(&format!("<<call send_email {SEND}>>"), &tools).unwrap();
    assert_eq!(got.calls[0].name, "send_email");
    for name in ["send_email3", "send_emai", "Send_email"] {
        assert_eq!(
            decode_calls(&format!("<<call {name} {SEND}>>"), &tools)
                .unwrap_err()
                .code(),
            "unknown_tool",
            "{name}"
        );
    }
}

#[test]
fn unicode_whitespace_before_close_marker() {
    for ws in ["\u{2003}", "\u{00A0}", "\u{3000}", " \n\t"] {
        let text = format!("<<call send_email {SEND}{ws}>>");
        assert_eq!(calls(&text).len(), 1, "{ws:?}");
    }
}

#[test]
fn split_markers_across_stream_chunks() {
    let got = stream(&[
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ])
    .unwrap();
    assert_eq!(
        got.calls,
        vec![call(
            "create_calendar_event",
            json!({"title":"Retro","start":"2026-10-04T10:00:00+05:30"})
        )]
    );
}

#[test]
fn every_split_point_gives_the_same_result() {
    let text =
        "ok <<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"नमस्ते >>\",\"body\":\"x\"}>> bye";
    let whole = decode_calls(text, &sample_tools()).unwrap();
    assert_eq!(whole.text, "ok  bye");
    let chars: Vec<char> = text.chars().collect();
    for split in 0..=chars.len() {
        let a: String = chars.iter().take(split).collect();
        let b: String = chars.iter().skip(split).collect();
        assert_eq!(stream(&[&a, &b]).unwrap(), whole, "split at char {split}");
    }
    let singles: Vec<String> = chars.iter().map(char::to_string).collect();
    let refs: Vec<&str> = singles.iter().map(String::as_str).collect();
    assert_eq!(stream(&refs).unwrap(), whole);
}

#[test]
fn stream_limit_fails_closed() {
    let text = format!("<<call send_email {SEND}>>");
    let mut d = StreamDecoder::with_limit(&sample_tools(), text.len());
    d.push(&text).unwrap();
    assert_eq!(
        d.finish().unwrap().calls.len(),
        1,
        "exactly at the limit is fine"
    );

    let mut d = StreamDecoder::with_limit(&sample_tools(), 10);
    d.push("0123456789").unwrap();
    assert_eq!(d.push("x").unwrap_err().code(), "invalid_arguments");
    assert_eq!(
        d.push("").unwrap_err().code(),
        "invalid_arguments",
        "stays failed"
    );
    assert_eq!(d.finish().unwrap_err().code(), "invalid_arguments");
}

#[test]
fn close_marker_inside_string_argument() {
    let got =
        calls(r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#);
    assert_eq!(got[0].arguments["subject"], json!("a >> b"));
}

#[test]
fn call_marker_inside_string_argument_is_data() {
    let got = decode_calls(
        r#"<<call send_email {"to":["a@b.c"],"subject":"<<call x {}>>","body":"<<call send_email {}>>"}>> after"#,
        &sample_tools(),
    )
    .unwrap();
    assert_eq!(got.calls.len(), 1);
    assert_eq!(got.calls[0].arguments["subject"], json!("<<call x {}>>"));
    assert_eq!(got.text, " after");
}

#[test]
fn unknown_tool() {
    assert_eq!(err_code("<<call delete_everything {}>>"), "unknown_tool");
}

#[test]
fn missing_required_title_and_bad_enum() {
    assert_eq!(
        err_code(r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#),
        "invalid_arguments"
    );
    assert_eq!(
        err_code(
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#
        ),
        "invalid_arguments"
    );
    assert_eq!(
        err_code(
            r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#
        ),
        "invalid_arguments"
    );
}

#[test]
fn wrong_types_never_coerce() {
    for args in [
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":"30"}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":30.5}"#,
        // Integer fields take integer literals only; 30.0 is not silently turned into 30.
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":30.0}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","duration_min":3e1}"#,
        r#"{"title":"x","start":"Monday 3pm"}"#,
        r#"{"title":null,"start":"2026-10-05T15:00:00+05:30"}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","attendees":"a@b.c"}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","attendees":[1]}"#,
        r#"{"title":"x","start":"2026-10-05T15:00:00+05:30","location":"HQ"}"#,
    ] {
        assert_eq!(
            err_code(&format!("<<call create_calendar_event {args}>>")),
            "invalid_arguments",
            "{args}"
        );
    }
}

#[test]
fn malformed_and_unterminated_calls() {
    for text in [
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}"#,
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}> >"#,
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","bo"#,
        r#"<<call send_email {"to":["a@b.c"],"subject":"s",}>>"#,
        r#"<<call {"to":[]}>>"#,
        "<<call",
        "<<call ",
        "<<call send_email",
        "text then <<call send_email ",
        // Whitespace after "call" means a call was intended; a missing "{" is a broken call.
        r#"<<call send_email: {"to":["a@b.c"],"subject":"s","body":"b"}>>"#,
        r#"<<call "send_email" {"to":["a@b.c"],"subject":"s","body":"b"}>>"#,
        r#"<<call send_email args {"to":["a@b.c"],"subject":"s","body":"b"}>>"#,
        "<<call send_email>> in a sentence",
        "<<call >>",
    ] {
        assert_eq!(err_code(text), "invalid_arguments", "{text}");
    }
}

#[test]
fn one_bad_call_fails_the_whole_reply() {
    let text = concat!(
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}>>"#,
        r#"<<call create_calendar_event {"title":"x"}>>"#
    );
    assert_eq!(err_code(text), "invalid_arguments");
}

#[test]
fn nested_object_and_array_args() {
    let tools = vec![nested_tool()];
    let args = json!({
        "title": "Disk full",
        "reporter": { "email": "a@b.c", "team": "infra" },
        "labels": ["p1", "ops"],
        "subtasks": [
            { "name": "clean /var", "estimate": 0.5, "done": false, "due": "2026-10-04" },
            { "name": "add alert" }
        ]
    });
    let text = format!("<<call create_ticket {args}>>");
    assert_eq!(
        decode_calls(&text, &tools).unwrap().calls,
        vec![call("create_ticket", args)]
    );

    for bad in [
        json!({"title":"t","reporter":{"team":"infra"}}),
        json!({"title":"t","reporter":{"email":"a","team":"mobile"}}),
        json!({"title":"t","reporter":{"email":"a"},"subtasks":[{"estimate":1}]}),
        json!({"title":"t","reporter":{"email":"a"},"subtasks":[{"name":"n","due":"tomorrow"}]}),
        json!({"title":"t","reporter":{"email":"a"},"subtasks":[{"name":"n","done":"yes"}]}),
        json!({"title":"t","reporter":{"email":"a","extra":1}}),
    ] {
        let text = format!("<<call create_ticket {bad}>>");
        assert_eq!(
            decode_calls(&text, &tools).unwrap_err().code(),
            "invalid_arguments",
            "{bad}"
        );
    }
}

#[test]
fn render_call_round_trips_with_escapes() {
    let c = call(
        "send_email",
        json!({"to":["q\"uote@x.y"],"subject":"a >> b <<call x {}>>","body":"line\nbreak\\ \u{1F600}"}),
    );
    let text = render_call(&c);
    assert_eq!(calls(&text), vec![c]);
}

#[test]
fn duplicate_keys_fail_closed() {
    for args in [
        r#"{"title":"a","title":"b","start":"2026-10-05T15:00:00Z"}"#,
        // The same key spelled with an escape.
        "{\"title\":\"a\",\"\\u0074itle\":\"b\",\"start\":\"2026-10-05T15:00:00Z\"}",
        r#"{"title":"a","start":"2026-10-05T15:00:00Z","attendees":["x"],"attendees":["y"]}"#,
    ] {
        let err = decode_calls(
            &format!("<<call create_calendar_event {args}>>"),
            &sample_tools(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_arguments", "{args}");
        assert!(err.to_string().contains("duplicate key"), "{err}");
    }
    // Same key in sibling objects, or as a string value, is not a duplicate.
    let tools = vec![nested_tool()];
    let ok = r#"<<call create_ticket {"title":"title","reporter":{"email":"title"},"subtasks":[{"name":"a"},{"name":"b"}]}>>"#;
    assert_eq!(decode_calls(ok, &tools).unwrap().calls.len(), 1);
    let nested_dup = r#"<<call create_ticket {"title":"t","reporter":{"email":"a","email":"b"}}>>"#;
    assert_eq!(
        decode_calls(nested_dup, &tools).unwrap_err().code(),
        "invalid_arguments"
    );
}

#[test]
fn limits_are_enforced_and_pattern_bypasses() {
    // `pattern` cannot be enforced without a regex engine, so it is never compacted.
    let with_pattern = common::tool(
        "p",
        json!({"type":"object","properties":{"s":{"type":"string","pattern":"^a+$"}}}),
    );
    let c = encode_tools(std::slice::from_ref(&with_pattern)).unwrap();
    assert!(!c.compacted());
    assert_eq!(c.bypass_reason().unwrap().as_label(), "unsupported_schema");
    // A caller that decodes against it anyway gets an error, never an unchecked call.
    assert!(decode_calls(r#"<<call p {"s":"zzz"}>>"#, &[with_pattern]).is_err());

    let tools = vec![common::tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "n": { "type": "integer", "minimum": 1, "maximum": 5 },
                "x": { "type": "number", "exclusiveMinimum": 0 },
                "s": { "type": "string", "maxLength": 3 },
                "l": { "type": "array", "items": { "type": "string" }, "maxItems": 2, "uniqueItems": true },
                "e": { "type": "string", "format": "email" }
            },
            "additionalProperties": false
        }),
    )];
    let run = |args: Value| decode_calls(&format!("<<call t {args}>>"), &tools);
    assert!(run(json!({"n":1,"x":0.1,"s":"zzz","l":["a","b"],"e":"not-an-email"})).is_ok());
    for bad in [
        json!({"n":0}),
        json!({"n":6}),
        json!({"x":0}),
        json!({"s":"abcd"}),
        json!({"s":"नमस्ते"}),
        json!({"l":["a","b","c"]}),
        json!({"l":["a","a"]}),
    ] {
        assert_eq!(
            run(bad.clone()).unwrap_err().code(),
            "invalid_arguments",
            "{bad}"
        );
    }
    // Lengths count characters, not bytes.
    assert!(run(json!({"s":"नमस"})).is_ok());
}

#[test]
fn integer_bounds_compare_exactly() {
    // 2^53 + 1 and 2^53 + 2 are the same f64; an f64 comparison would let the second through.
    let tools = vec![common::tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "n": { "type": "integer", "maximum": 9_007_199_254_740_993_u64 },
                "m": { "type": "integer", "exclusiveMinimum": -9_007_199_254_740_993_i64 },
                "x": { "type": "number", "maximum": 1.5 }
            }
        }),
    )];
    let run = |args: Value| decode_calls(&format!("<<call t {args}>>"), &tools);
    assert!(run(json!({"n": 9_007_199_254_740_993_u64})).is_ok());
    assert!(run(json!({"n": 9_007_199_254_740_994_u64})).is_err());
    assert!(run(json!({"m": -9_007_199_254_740_992_i64})).is_ok());
    assert!(run(json!({"m": -9_007_199_254_740_993_i64})).is_err());
    // Mixed integer/float still compares numerically.
    assert!(run(json!({"x": 1})).is_ok());
    assert!(run(json!({"x": 2})).is_err());
}
