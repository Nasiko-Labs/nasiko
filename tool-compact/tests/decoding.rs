//! The decoder's failure-first matrix, through the public API only.

mod common;

use common::{args, tools};
use nasiko_tool_compact::{
    ArgumentFault, CallFault, CompactError, StreamDecoder, decode_calls, decode_reply,
};
use serde_json::json;

fn code(text: &str) -> &'static str {
    decode_calls(text, &tools()).unwrap_err().code()
}

#[test]
fn single_call_decodes_to_validated_arguments() {
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
    assert_eq!(
        calls[0].arguments,
        args(json!({"title": "Design review", "start": "2026-10-05T15:00:00+05:30"}))
    );
}

#[test]
fn multiple_calls_keep_their_order_including_the_same_tool_twice() {
    let text = concat!(
        r#"<<call send_email {"to":["a@x.io"],"subject":"s","body":"b"}>>"#,
        "\n",
        r#"<<call ping>>"#,
        "\n",
        r#"<<call send_email {"to":["b@x.io"],"subject":"t","body":"c"}>>"#
    );
    let names: Vec<_> = decode_calls(text, &tools())
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["send_email", "ping", "send_email"]);
}

#[test]
fn text_before_between_and_after_calls_is_returned_and_never_corrupts_decoding() {
    let decoded = decode_reply(
        "Sure — booking it now.\n<<call ping>> then a < b and a << b.\nDone.",
        &tools(),
    )
    .unwrap();
    assert_eq!(decoded.calls.len(), 1);
    assert_eq!(
        decoded.text,
        "Sure — booking it now.\n then a < b and a << b.\nDone."
    );
}

#[test]
fn a_reply_without_calls_stays_a_plain_reply() {
    let decoded = decode_reply("I can't check the weather, sorry.", &tools()).unwrap();
    assert!(decoded.calls.is_empty());
    assert_eq!(decoded.text, "I can't check the weather, sorry.");
    assert!(decode_calls("", &tools()).unwrap().is_empty());
}

#[test]
fn marker_text_inside_string_arguments_does_not_end_the_call() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["s@x.io"],"subject":"a >> b }>> <<call x","body":"\"}>> \\ done"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls[0].arguments["subject"], json!("a >> b }>> <<call x"));
    assert_eq!(calls[0].arguments["body"], json!("\"}>> \\ done"));
}

#[test]
fn unicode_escapes_and_raw_unicode_survive() {
    let calls = decode_calls(
        r#"<<call send_email {"to":["z@x.io"],"subject":"会议 😀","body":"café\nline"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls[0].arguments["subject"], json!("会议 😀"));
    assert_eq!(calls[0].arguments["body"], json!("café\nline"));
}

#[test]
fn nested_objects_and_arrays_validate_recursively() {
    let ok = r#"<<call tracker.create_ticket {"title":"500s","priority":"high","assignee":{"name":"Dana","email":"d@x.io"},"labels":["auth"]}>>"#;
    assert_eq!(decode_calls(ok, &tools()).unwrap().len(), 1);

    let missing_nested = r#"<<call tracker.create_ticket {"title":"t","priority":"low","assignee":{"email":"d@x.io"}}>>"#;
    assert!(matches!(
        decode_calls(missing_nested, &tools()).unwrap_err(),
        CompactError::InvalidArguments { path, fault: ArgumentFault::MissingRequired, .. } if path == "/assignee/name"
    ));
}

#[test]
fn unknown_tools_fail_closed_and_similar_names_are_never_matched() {
    for name in [
        "delete_everything",
        "send_mail",
        "Send_Email",
        "functions.send_email",
        "send_email_v2",
    ] {
        let text = format!(r#"<<call {name} {{"to":["a@x.io"],"subject":"s","body":"b"}}>>"#);
        assert_eq!(
            decode_calls(&text, &tools()).unwrap_err(),
            CompactError::UnknownTool { name: name.into() },
            "{name}"
        );
    }
}

#[test]
fn an_unknown_tool_is_reported_even_when_its_arguments_are_also_broken() {
    assert_eq!(code(r#"<<call nope {"a":1,,,"#), "unknown_tool");
}

#[test]
fn a_declared_dotted_tool_name_decodes_exactly() {
    let calls = decode_calls(
        r#"<<call tracker.create_ticket {"title":"t","priority":"medium"}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(calls[0].name, "tracker.create_ticket");
}

#[test]
fn argument_faults_fail_closed_never_repaired() {
    let cases = [
        // missing required
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30"}>>"#,
        // enum violation (case, whitespace, unknown value)
        r#"<<call create_calendar_event {"title":"t","start":"s","visibility":"secret"}>>"#,
        r#"<<call create_calendar_event {"title":"t","start":"s","visibility":"Private"}>>"#,
        // wrong primitive types
        r#"<<call create_calendar_event {"title":"t","start":"s","duration_min":"30"}>>"#,
        r#"<<call create_calendar_event {"title":"t","start":"s","duration_min":1.5}>>"#,
        r#"<<call create_calendar_event {"title":5,"start":"s"}>>"#,
        r#"<<call send_email {"to":"a@x.io","subject":"s","body":"b"}>>"#,
        // undeclared argument
        r#"<<call create_calendar_event {"title":"t","start":"s","location":"HQ"}>>"#,
        // malformed JSON
        r#"<<call create_calendar_event {"title":"t","start":"s",}>>"#,
        r#"<<call create_calendar_event {'title':'t','start':'s'}>>"#,
        r#"<<call create_calendar_event {"title":"t","title":"u","start":"s"}>>"#,
        r#"<<call create_calendar_event {"title":"t","start":NaN}>>"#,
        // maxItems
        r#"<<call tracker.create_ticket {"title":"t","priority":"low","labels":["a","b","c","d","e","f"]}>>"#,
    ];
    for text in cases {
        assert_eq!(code(text), "invalid_arguments", "{text}");
    }
}

#[test]
fn an_integer_written_with_a_zero_fraction_is_accepted_unchanged() {
    let calls = decode_calls(
        r#"<<call create_calendar_event {"title":"t","start":"s","duration_min":30.0}>>"#,
        &tools(),
    )
    .unwrap();
    assert_eq!(
        calls[0].arguments_json(),
        r#"{"duration_min":30.0,"start":"s","title":"t"}"#
    );
}

#[test]
fn broken_markers_are_errors_not_text() {
    let cases = [
        ("<<call>>", CallFault::EmptyName),
        ("<<call {}>>", CallFault::EmptyName),
        (r#"<<call ping {"a":1} >"#, CallFault::Unterminated),
        (r#"<<call ping {} > >"#, CallFault::MissingClose),
        (r#"<<call ping {}"#, CallFault::Unterminated),
        (r#"<<call ping {"a""#, CallFault::Unterminated),
        ("<<call ping", CallFault::Unterminated),
        ("<<call ", CallFault::Unterminated),
        ("<<call ping x>>", CallFault::UnexpectedCharacter('x')),
    ];
    for (text, fault) in cases {
        assert_eq!(
            decode_calls(text, &tools()).unwrap_err(),
            CompactError::MalformedCall { fault },
            "{text}"
        );
    }
}

#[test]
fn valid_json_without_the_closing_marker_is_not_accepted() {
    assert_eq!(code(r#"<<call ping {}"#), "invalid_arguments");
    assert_eq!(
        code(r#"<<call create_calendar_event {"title":"t","start":"s"}"#),
        "invalid_arguments"
    );
}

#[test]
fn look_alike_markers_stay_plain_text() {
    for text in [
        "<<callback>>",
        "<<CALL ping>>",
        "<<cal ping>>",
        "<< call ping >>",
        "<<call",
    ] {
        let decoded = decode_reply(text, &tools()).unwrap();
        assert!(decoded.calls.is_empty(), "{text}");
        assert_eq!(decoded.text, text);
    }
}

#[test]
fn whitespace_inside_the_marker_may_be_tabs_and_newlines() {
    let text = "<<call\tcreate_calendar_event\r\n{\"title\": \"t\",\n \"start\": \"s\"}\n>>";
    assert_eq!(decode_calls(text, &tools()).unwrap().len(), 1);
}

#[test]
fn omitted_arguments_mean_an_empty_object() {
    assert!(
        decode_calls("<<call ping>>", &tools()).unwrap()[0]
            .arguments
            .is_empty()
    );
    assert_eq!(
        code("<<call create_calendar_event>>"),
        "invalid_arguments",
        "an empty object still has to satisfy `required`"
    );
}

#[test]
fn a_tool_without_parameters_rejects_any_argument() {
    assert_eq!(code(r#"<<call ping {"x":1}>>"#), "invalid_arguments");
}

#[test]
fn a_bad_second_call_voids_the_valid_first_call() {
    let text = r#"<<call ping>> and then <<call create_calendar_event {"title":"t"}>>"#;
    assert_eq!(code(text), "invalid_arguments");
}

#[test]
fn an_oversized_call_is_rejected_with_too_large() {
    let huge = "a".repeat(nasiko_tool_compact::MAX_CALL_BYTES + 10);
    let text = format!(r#"<<call send_email {{"to":["a@x.io"],"subject":"s","body":"{huge}"}}>>"#);
    assert_eq!(
        decode_calls(&text, &tools()).unwrap_err(),
        CompactError::MalformedCall {
            fault: CallFault::TooLarge
        }
    );
}

#[test]
fn calls_inside_a_code_fence_decode_and_the_fence_stays_text() {
    let decoded = decode_reply("```\n<<call ping>>\n```", &tools()).unwrap();
    assert_eq!(decoded.calls.len(), 1);
    assert_eq!(decoded.text, "```\n\n```");
}

#[test]
fn a_poisoned_decoder_keeps_failing() {
    let mut decoder = StreamDecoder::new(&tools()).unwrap();
    let first = decoder.push("<<call nope ").unwrap_err();
    assert_eq!(decoder.push("text").unwrap_err(), first);
    assert_eq!(decoder.finish().unwrap_err(), first);
}

#[test]
fn duplicate_tool_names_cannot_be_decoded_against() {
    let mut twice = tools();
    twice.push(common::ping());
    assert!(matches!(
        StreamDecoder::new(&twice),
        Err(CompactError::Unsupported { .. })
    ));
}
