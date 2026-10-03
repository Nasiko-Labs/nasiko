mod common;

use common::*;
use nasiko_tool_compact::{Error, ToolDef, decode_calls, render_call};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn err_code(text: &str) -> &'static str {
    decode_calls(text, &tools())
        .expect_err(&format!("must be rejected: {text}"))
        .code()
}

// ── happy paths ─────────────────────────────────────────────────────────────

#[test]
fn single_call() {
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"Meeting","start":"2026-10-05T15:00:00+05:30","visibility":"public"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(
        args(&calls[0]),
        json!({"title":"Meeting","start":"2026-10-05T15:00:00+05:30","visibility":"public"})
    );
}

#[test]
fn no_call_is_an_empty_list() {
    assert!(
        decode_calls("It is sunny in Pune.", &tools())
            .unwrap()
            .is_empty()
    );
    assert!(decode_calls("", &tools()).unwrap().is_empty());
}

#[test]
fn text_before_and_after_calls_is_ignored() {
    let text = "Sure, booking it.\n<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>>\nDone!";
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "send_email");
}

#[test]
fn multiple_calls_keep_order() {
    let text = concat!(
        r#"<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"ok"}>>"#,
        "\nand then\n",
        r#"<<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30}>>"#,
    );
    let calls = decode_calls(text, &tools()).unwrap();
    let names: Vec<_> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["send_email", "create_calendar_event"]);
}

#[test]
fn whitespace_inside_marker_is_tolerated() {
    let text = "<<call   send_email \n {\"to\":[],\"subject\":\"s\",\"body\":\"b\"} \n >>";
    assert_eq!(decode_calls(text, &tools()).unwrap().len(), 1);
}

#[test]
fn gt_gt_inside_a_json_string_does_not_end_the_call() {
    let text = r#"<<call send_email {"to":["a@b.c"],"subject":"a >> b","body":"}>> \" >>"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(args(&calls[0])["subject"], "a >> b");
    assert_eq!(args(&calls[0])["body"], "}>> \" >>");
}

#[test]
fn prose_that_merely_looks_like_a_marker_is_not_a_call() {
    for text in [
        "a << b >> c",
        "<<callous>>",
        "<<call>>",
        "<<calls here>>",
        "<< call send_email {}>>",
        "call send_email {}",
        "<<",
        "<<ca",
    ] {
        assert!(decode_calls(text, &tools()).unwrap().is_empty(), "{text}");
    }
}

#[test]
fn arguments_are_emitted_as_canonical_json_string() {
    let calls = decode_calls(
        "<<call send_email { \"to\" : [ ] , \"subject\":\"s\",\"body\":\"नमस्ते\" }>>",
        &tools(),
    )
    .unwrap();
    assert_eq!(
        calls[0].arguments,
        r#"{"body":"नमस्ते","subject":"s","to":[]}"#
    );
}

#[test]
fn render_then_decode_round_trips() {
    let a = json!({"title":"a >> \"b\"","start":"2026-10-05T15:00:00Z","attendees":["x@y.z"],"duration_min":5});
    let text = render_call("create_calendar_event", &a);
    assert!(text.starts_with("<<call create_calendar_event {"));
    assert!(text.ends_with("}>>"));
    let calls = decode_calls(&text, &tools()).unwrap();
    assert_eq!(args(&calls[0]), a);
}

// ── fail closed ─────────────────────────────────────────────────────────────

#[test]
fn unknown_tool_is_rejected() {
    assert_eq!(err_code("<<call delete_everything {}>>"), "unknown_tool");
    // Near misses are not silently renamed.
    assert_eq!(
        err_code(r#"<<call Send_Email {"to":[],"subject":"s","body":"b"}>>"#),
        "unknown_tool"
    );
}

#[test]
fn missing_required_argument_is_rejected() {
    assert_eq!(
        err_code(r#"<<call create_calendar_event {"start":"2026-10-02T10:00:00Z"}>>"#),
        "invalid_arguments"
    );
}

#[test]
fn invalid_enum_is_rejected() {
    assert_eq!(
        err_code(
            r#"<<call create_calendar_event {"title":"t","start":"2026-10-02T10:00:00Z","visibility":"secret"}>>"#
        ),
        "invalid_arguments"
    );
    // Enum matching is exact — no case folding.
    assert_eq!(
        err_code(
            r#"<<call create_calendar_event {"title":"t","start":"2026-10-02T10:00:00Z","visibility":"Public"}>>"#
        ),
        "invalid_arguments"
    );
}

#[test]
fn invalid_types_are_rejected() {
    let base = |extra: &str| {
        format!(
            r#"<<call create_calendar_event {{"title":"t","start":"2026-10-02T10:00:00Z",{extra}}}>>"#
        )
    };
    for bad in [
        r#""duration_min":"30""#,
        r#""duration_min":30.5"#,
        r#""duration_min":30.0"#,
        r#""duration_min":null"#,
        r#""duration_min":true"#,
        r#""attendees":"a@b.c""#,
        r#""attendees":[1]"#,
        r#""attendees":null"#,
        r#""visibility":1"#,
    ] {
        assert_eq!(err_code(&base(bad)), "invalid_arguments", "{bad}");
    }
    for bad_title in [
        r#"{"title":1,"start":"2026-10-02T10:00:00Z"}"#,
        r#"{"title":["a"],"start":"2026-10-02T10:00:00Z"}"#,
    ] {
        assert_eq!(
            err_code(&format!("<<call create_calendar_event {bad_title}>>")),
            "invalid_arguments"
        );
    }
}

#[test]
fn datetime_must_be_rfc3339() {
    for bad in [
        "tomorrow",
        "2026-10-05",
        "2026-10-05 15:00:00+05:30",
        "2026-10-05T15:00:00",
        "2026-13-05T15:00:00Z",
        "2026-02-30T15:00:00Z",
        "2026-10-05T25:00:00Z",
        "2026-10-05T15:00:00+5:30",
    ] {
        let t = format!(r#"<<call create_calendar_event {{"title":"t","start":"{bad}"}}>>"#);
        assert_eq!(err_code(&t), "invalid_arguments", "{bad}");
    }
    for ok in [
        "2026-10-05T15:00:00Z",
        "2026-10-05T15:00:00.123+05:30",
        "2024-02-29T00:00:00-08:00",
        "2026-10-05t15:00:00z",
    ] {
        let t = format!(r#"<<call create_calendar_event {{"title":"t","start":"{ok}"}}>>"#);
        assert!(decode_calls(&t, &tools()).is_ok(), "{ok}");
    }
}

#[test]
fn unknown_argument_names_are_rejected_not_dropped() {
    assert_eq!(
        err_code(r#"<<call send_email {"to":[],"subject":"s","body":"b","bcc":["x@y.z"]}>>"#),
        "invalid_arguments"
    );
}

#[test]
fn nested_objects_and_arrays_are_validated() {
    let t = tool(
        "ship",
        None,
        Some(json!({"type":"object","properties":{
            "addr":{"type":"object","properties":{"city":{"type":"string"},"zip":{"type":"integer"}},"required":["city"]},
            "items":{"type":"array","items":{"type":"object","properties":{"sku":{"type":"string"}},"required":["sku"]}}},
            "required":["addr"]})),
    );
    let ts = [t];
    let ok = r#"<<call ship {"addr":{"city":"Pune"},"items":[{"sku":"a"},{"sku":"b"}]}>>"#;
    assert!(decode_calls(ok, &ts).is_ok());
    for bad in [
        r#"<<call ship {"addr":{}}>>"#,
        r#"<<call ship {"addr":{"city":"Pune","zip":"411"}}>>"#,
        r#"<<call ship {"addr":{"city":"Pune","x":1}}>>"#,
        r#"<<call ship {"addr":{"city":"Pune"},"items":[{"sku":"a"},{}]}>>"#,
        r#"<<call ship {"addr":"Pune"}>>"#,
    ] {
        assert_eq!(
            decode_calls(bad, &ts).unwrap_err().code(),
            "invalid_arguments",
            "{bad}"
        );
    }
}

#[test]
fn malformed_json_arguments_are_an_error() {
    for bad in [
        r#"<<call send_email {"to":}>>"#,
        r#"<<call send_email {'to':[]}>>"#,
        r#"<<call send_email {"to":[],}>>"#,
        r#"<<call send_email {"to":[] "subject":"s"}>>"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"]>>"#,
        r#"<<call send_email {"to":[1,]}>>"#,
    ] {
        let e = decode_calls(bad, &tools()).expect_err(bad);
        assert!(
            matches!(e, Error::InvalidArguments { .. } | Error::MalformedCall(_)),
            "{bad}: {e:?}"
        );
    }
}

#[test]
fn malformed_markers_are_errors_never_panics() {
    for bad in [
        "<<call",
        "<<call ",
        "<<call send_email",
        "<<call send_email {",
        r#"<<call send_email {"to":[],"subject":"s","body":"b"}"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"}>"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"}> >"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"} x>>"#,
        r#"<<call send_email {"to":[],"subject":"unterminated}>>"#,
        r#"<<call send_email>>"#,
        r#"<<call send_email [1]>>"#,
        r#"<<call send_email{"to":[],"subject":"s","body":"b"}>>"#,
        r#"<<call se!nd {}>>"#,
        r#"<<call  {}>>"#,
    ] {
        assert!(decode_calls(bad, &tools()).is_err(), "must reject: {bad:?}");
    }
}

#[test]
fn one_bad_call_rejects_the_whole_response() {
    let text = r#"<<call send_email {"to":[],"subject":"s","body":"b"}>> <<call nope {}>>"#;
    assert_eq!(
        decode_calls(text, &tools()).unwrap_err().code(),
        "unknown_tool"
    );
}

#[test]
fn calling_a_bypassed_tool_in_compact_form_is_rejected() {
    let odd = tool(
        "odd",
        None,
        Some(json!({"type":"object","properties":{"x":{"type":"string","minLength":2}}})),
    );
    let e = decode_calls(r#"<<call odd {"x":"ab"}>>"#, &[odd]).unwrap_err();
    assert_eq!(e.code(), "unsupported_schema");
}

#[test]
fn duplicate_tools_are_rejected_at_decode_time() {
    let e = decode_calls("hi", &[calendar(), calendar()]).unwrap_err();
    assert_eq!(e.code(), "duplicate_tool");
}

#[test]
fn error_codes_match_the_eval_contract() {
    assert_eq!(Error::UnknownTool("x".into()).code(), "unknown_tool");
    assert_eq!(
        Error::InvalidArguments {
            tool: "t".into(),
            reason: "r".into()
        }
        .code(),
        "invalid_arguments"
    );
}

#[test]
fn duplicate_keys_are_rejected_not_resolved_last_wins() {
    for bad in [
        r#"<<call send_email {"to":[],"subject":"a","subject":"b","body":"x"}>>"#,
        r#"<<call create_calendar_event {"title":"t","start":"2026-10-05T15:00:00Z","attendees":["a"],"attendees":["b"]}>>"#,
    ] {
        assert_eq!(err_code(bad), "invalid_arguments", "{bad}");
    }
}

#[test]
fn duplicate_keys_in_nested_objects_are_rejected() {
    let t = tool(
        "ship",
        None,
        Some(
            json!({"type":"object","properties":{"addr":{"type":"object","properties":{"city":{"type":"string"}}}}}),
        ),
    );
    let bad = r#"<<call ship {"addr":{"city":"a","city":"b"}}>>"#;
    assert_eq!(
        decode_calls(bad, &[t]).unwrap_err().code(),
        "invalid_arguments"
    );
}

// ── regression guards for failure modes found while reviewing the decoder ───

#[test]
fn marker_text_inside_a_string_argument_is_data_not_a_second_call() {
    let text = r#"<<call send_email {"to":[],"subject":"<<call send_email {}>>","body":"<<call nope {}>> >>"}>>"#;
    let calls = decode_calls(text, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(args(&calls[0])["subject"], "<<call send_email {}>>");
    assert_eq!(args(&calls[0])["body"], "<<call nope {}>> >>");
}

#[test]
fn pathologically_deep_json_is_an_error_not_a_stack_overflow() {
    for depth in [200, 5_000, 200_000] {
        let nested = format!(
            "{}{}",
            "{\"a\":".repeat(depth),
            "1".to_string() + &"}".repeat(depth)
        );
        let text = format!("<<call send_email {nested}>>");
        let e = decode_calls(&text, &tools()).expect_err("deep nesting must be rejected");
        assert_eq!(e.code(), "invalid_arguments", "depth {depth}");
        let arrays = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        let text = format!(r#"<<call send_email {{"to":{arrays},"subject":"s","body":"b"}}>>"#);
        assert!(decode_calls(&text, &tools()).is_err(), "depth {depth}");
    }
}

#[test]
fn zero_parameter_tool_accepts_empty_args_and_rejects_extras() {
    let ping = [tool("ping", Some("Check liveness"), None)];
    assert_eq!(
        decode_calls("<<call ping {}>>", &ping).unwrap()[0].arguments,
        "{}"
    );
    assert_eq!(
        decode_calls(r#"<<call ping {"x":1}>>"#, &ping)
            .unwrap_err()
            .code(),
        "invalid_arguments"
    );
}
