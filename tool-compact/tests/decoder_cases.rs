//! Decoder behaviour: the public sample's decoder cases plus edge cases of our own.
//!
//! The sample is a development set; nothing here special-cases its ids, strings or tools.

use nasiko_tool_compact::{DecodeError, StreamDecoder, ToolCall, ToolDef, decode_calls};
use serde_json::{Value, json};

const SAMPLE: &str = include_str!("fixtures/sample.json");

fn sample_tools() -> Vec<ToolDef> {
    let data: Value = serde_json::from_str(SAMPLE).unwrap();
    data["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| ToolDef {
            name: t["function"]["name"].as_str().unwrap().into(),
            description: t["function"]["description"].as_str().map(String::from),
            parameters: t["function"].get("parameters").cloned(),
        })
        .collect()
}

fn feed(tools: &[ToolDef], chunks: &[&str]) -> Result<Vec<ToolCall>, DecodeError> {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();
    for chunk in chunks {
        calls.extend(decoder.push(chunk)?);
    }
    calls.extend(decoder.finish()?);
    Ok(calls)
}

fn as_json(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|c| {
                json!({"name": c.name, "arguments": serde_json::from_str::<Value>(&c.arguments).unwrap()})
            })
            .collect(),
    )
}

fn err_code(result: Result<Vec<ToolCall>, DecodeError>) -> &'static str {
    result.expect_err("expected an error").code()
}

#[test]
fn public_decoder_cases() {
    let tools = sample_tools();
    let data: Value = serde_json::from_str(SAMPLE).unwrap();
    for case in data["decoder_cases"].as_array().unwrap() {
        let chunks: Vec<&str> = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        let got = feed(&tools, &chunks);
        let expected = &case["expected"];
        match expected.get("error").and_then(Value::as_str) {
            Some(code) => assert_eq!(got.unwrap_err().code(), code, "{}", case["id"]),
            None => assert_eq!(as_json(&got.unwrap()), expected["calls"], "{}", case["id"]),
        }
    }
}

const CAL: &str =
    r#"<<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00+05:30"}>>"#;

#[test]
fn no_marker_is_a_plain_answer() {
    let tools = sample_tools();
    assert_eq!(decode_calls("The build is green.", &tools).unwrap(), vec![]);
    assert_eq!(decode_calls("", &tools).unwrap(), vec![]);
    assert_eq!(decode_calls("a << b and c > d", &tools).unwrap(), vec![]);
}

#[test]
fn text_before_and_after_a_call_is_ignored() {
    let tools = sample_tools();
    let text = format!("Sure, booking it now.\n{CAL}\nAnything else?");
    let calls = decode_calls(&text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "create_calendar_event");
}

#[test]
fn two_calls_keep_their_order() {
    let tools = sample_tools();
    let text = format!(
        "{CAL}\n<<call send_email {{\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}}>>"
    );
    let names: Vec<String> = decode_calls(&text, &tools)
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["create_calendar_event", "send_email"]);
}

#[test]
fn arguments_are_canonical_sorted_compact_json() {
    let tools = sample_tools();
    let text = r#"<<call send_email { "x": 1 }>>"#;
    // Whitespace before the JSON object is not part of the grammar.
    assert_eq!(err_code(decode_calls(text, &tools)), "invalid_arguments");
    let text = r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b","cc":[]}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(
        calls[0].arguments,
        r#"{"body":"b","cc":[],"subject":"s","to":["a@b.c"]}"#
    );
}

#[test]
fn unknown_tool_is_rejected() {
    let tools = sample_tools();
    assert_eq!(
        err_code(decode_calls("<<call nope {}>>", &tools)),
        "unknown_tool"
    );
}

#[test]
fn invalid_arguments_are_rejected_not_repaired() {
    let tools = sample_tools();
    let cases = [
        // missing required
        r#"<<call send_email {"to":["a@b.c"],"subject":"s"}>>"#,
        // wrong type
        r#"<<call send_email {"to":"a@b.c","subject":"s","body":"b"}>>"#,
        // array item of the wrong type
        r#"<<call send_email {"to":[1],"subject":"s","body":"b"}>>"#,
        // enum violation
        r#"<<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#,
        // bad datetime
        r#"<<call create_calendar_event {"title":"T","start":"tomorrow"}>>"#,
        r#"<<call create_calendar_event {"title":"T","start":"2026-13-05T15:00:00+05:30"}>>"#,
        // integer given as float or string
        r#"<<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00Z","duration_min":30.5}>>"#,
        r#"<<call create_calendar_event {"title":"T","start":"2026-10-05T15:00:00Z","duration_min":"30"}>>"#,
        // trailing comma, single quotes, unquoted keys
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b",}>>"#,
        r#"<<call send_email {'to':['a'],'subject':'s','body':'b'}>>"#,
        r#"<<call send_email {to:["a"],subject:"s",body:"b"}>>"#,
        // duplicate key is ambiguous
        r#"<<call send_email {"to":["a"],"subject":"s","subject":"t","body":"b"}>>"#,
        // non-object arguments
        r#"<<call send_email ["a"]>>"#,
        r#"<<call send_email "x">>"#,
        // extra closer or junk after the object
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"}}>>"#,
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"} >>"#,
        // missing space, double space, newline instead of space
        r#"<<call send_email{"to":["a"],"subject":"s","body":"b"}>>"#,
        r#"<<call  send_email {"to":["a"],"subject":"s","body":"b"}>>"#,
        "<<call send_email\n{\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"}>>",
    ];
    for text in cases {
        let got = decode_calls(text, &sample_tools());
        assert_eq!(
            got.map_err(|e| e.code()),
            Err("invalid_arguments"),
            "{text}"
        );
    }
    let _ = tools;
}

#[test]
fn truncated_calls_are_errors_at_finish() {
    let tools = sample_tools();
    for cut in [
        "<<call send_email",
        "<<call send_email ",
        "<<call send_email {\"to\":[\"a\"",
        "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"}",
        "<<call send_email {\"to\":[\"a\"],\"subject\":\"s\",\"body\":\"b\"}>",
    ] {
        assert_eq!(
            err_code(decode_calls(cut, &tools)),
            "invalid_arguments",
            "{cut}"
        );
    }
}

#[test]
fn a_bare_partial_marker_at_the_end_is_plain_text() {
    let tools = sample_tools();
    // Fewer characters than the full `<<call ` opener: not a call, nothing is guessed.
    for text in ["ok <", "ok <<", "ok <<ca", "ok <<call"] {
        assert_eq!(decode_calls(text, &tools).unwrap(), vec![], "{text}");
    }
}

#[test]
fn escapes_unicode_and_gt_gt_inside_strings() {
    let tools = sample_tools();
    let text = r#"<<call send_email {"to":["a@b.c"],"subject":"q \" }}>> \\ é 😀 é","body":"line1\nline2 >> {"}>>"#;
    let calls = decode_calls(text, &tools).unwrap();
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["subject"], "q \" }}>> \\ é 😀 é");
    assert_eq!(args["body"], "line1\nline2 >> {");
}

#[test]
fn marker_split_at_every_character_still_decodes() {
    let tools = sample_tools();
    let text = format!("hi {CAL} bye");
    let expected = decode_calls(&text, &tools).unwrap();
    let chars: Vec<char> = text.chars().collect();
    let singles: Vec<String> = chars.iter().map(|c| c.to_string()).collect();
    let refs: Vec<&str> = singles.iter().map(String::as_str).collect();
    assert_eq!(feed(&tools, &refs).unwrap(), expected);
}

#[test]
fn an_error_poisons_the_decoder() {
    let tools = sample_tools();
    let mut decoder = StreamDecoder::new(&tools);
    let first = decoder.push("<<call nope {}>>").unwrap_err();
    assert_eq!(first.code(), "unknown_tool");
    // Later input, even a valid call, never yields a call.
    assert_eq!(decoder.push(CAL).unwrap_err(), first);
    assert_eq!(decoder.finish().unwrap_err(), first);
}

#[test]
fn each_call_is_emitted_exactly_once() {
    let tools = sample_tools();
    let mut decoder = StreamDecoder::new(&tools);
    let text = format!("{CAL}{CAL}");
    let (a, b) = text.split_at(text.len() - 10);
    let mut total = 0;
    total += decoder.push(a).unwrap().len();
    assert_eq!(total, 1, "first call is out as soon as it closes");
    total += decoder.push(b).unwrap().len();
    total += decoder.finish().unwrap().len();
    assert_eq!(total, 2);
}

#[test]
fn oversized_calls_are_rejected() {
    let tools = sample_tools();
    let big = "x".repeat(70 * 1024);
    let text = format!("<<call send_email {{\"to\":[],\"subject\":\"{big}\",\"body\":\"b\"}}>>");
    assert_eq!(err_code(decode_calls(&text, &tools)), "invalid_arguments");
}

#[test]
fn additional_properties_false_is_enforced() {
    let tools = vec![ToolDef {
        name: "t".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"a": {"type": "string"}},
            "additionalProperties": false
        })),
    }];
    assert!(decode_calls(r#"<<call t {"a":"x"}>>"#, &tools).is_ok());
    assert_eq!(
        err_code(decode_calls(r#"<<call t {"a":"x","b":1}>>"#, &tools)),
        "invalid_arguments"
    );
    // Without the keyword, extra properties are accepted.
    let open = vec![ToolDef {
        name: "t".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {"a": {"type": "string"}}})),
    }];
    assert!(decode_calls(r#"<<call t {"a":"x","b":1}>>"#, &open).is_ok());
}

#[test]
fn nested_objects_and_arrays_are_validated() {
    let tools = vec![ToolDef {
        name: "book".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "guest": {"type": "object", "properties": {"name": {"type": "string"}, "age": {"type": "integer"}}, "required": ["name"]},
                "rooms": {"type": "array", "items": {"type": "object", "properties": {"beds": {"type": "number"}}, "required": ["beds"]}}
            },
            "required": ["guest"]
        })),
    }];
    assert!(
        decode_calls(
            r#"<<call book {"guest":{"name":"n","age":3},"rooms":[{"beds":1.5},{"beds":2}]}>>"#,
            &tools
        )
        .is_ok()
    );
    for bad in [
        r#"<<call book {"guest":{"age":3}}>>"#,
        r#"<<call book {"guest":{"name":"n","age":"3"}}>>"#,
        r#"<<call book {"guest":{"name":"n"},"rooms":[{"beds":"x"}]}>>"#,
        r#"<<call book {"guest":{"name":"n"},"rooms":[{}]}>>"#,
    ] {
        assert_eq!(
            err_code(decode_calls(bad, &tools)),
            "invalid_arguments",
            "{bad}"
        );
    }
}

#[test]
fn tools_without_parameters_accept_an_empty_object() {
    let tools = vec![ToolDef {
        name: "ping".into(),
        description: None,
        parameters: None,
    }];
    assert_eq!(decode_calls("<<call ping {}>>", &tools).unwrap().len(), 1);
}
