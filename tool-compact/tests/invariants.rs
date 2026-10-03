//! The crate's invariants, exercised through the public API only.
//!
//! These are the tests that must still pass when the grammar grows — they pin behaviour a caller
//! relies on, not how it is implemented.

use nasiko_tool_compact::{
    CompactError, Event, StreamDecoder, ToolCall, ToolDef, decode, decode_calls, decode_tools,
    encode_tools, render_calls,
};
use serde_json::{Value, json};

fn calendar() -> ToolDef {
    ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })),
    }
}

fn email() -> ToolDef {
    ToolDef {
        name: "send_email".into(),
        description: Some("Send an email from the user's account.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                "subject": {"type": "string", "description": "Subject line"},
                "body": {"type": "string", "description": "Plain-text body"},
                "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
            },
            "required": ["to", "subject", "body"]
        })),
    }
}

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn stream(chunks: &[&str], tools: &[ToolDef]) -> Result<Vec<ToolCall>, CompactError> {
    let mut decoder = StreamDecoder::new(tools)?;
    let mut events = Vec::new();
    for chunk in chunks {
        events.extend(decoder.push(chunk)?);
    }
    events.extend(decoder.finish()?);
    Ok(events
        .into_iter()
        .filter_map(|event| match event {
            Event::Call(call) => Some(call),
            Event::Text(_) => None,
        })
        .collect())
}

fn arguments(call: &ToolCall) -> Value {
    serde_json::from_str(&call.arguments).unwrap()
}

// ── schema meaning ──────────────────────────────────────────────────────────────────────────

#[test]
fn schemas_survive_encoding_exactly() {
    let compact = encode_tools(&tools()).unwrap();
    assert_eq!(decode_tools(&compact).unwrap(), tools());
}

#[test]
fn the_compact_form_is_smaller_than_the_schema_it_replaces() {
    let native: usize = tools()
        .iter()
        .map(|t| serde_json::to_string(t).unwrap().len())
        .sum();
    let compact = encode_tools(&tools()).unwrap().prompt().len();
    assert!(compact < native, "{compact} bytes vs {native} native");
}

#[test]
fn an_unsupported_tool_is_refused_so_the_caller_can_bypass() {
    let mut odd = calendar();
    odd.parameters = Some(json!({
        "type": "object",
        "properties": {"when": {"oneOf": [{"type": "string"}, {"type": "integer"}]}}
    }));
    let error = encode_tools(&[email(), odd.clone()]).unwrap_err();
    assert!(matches!(error, CompactError::Unsupported { ref tool, .. } if tool == &odd.name));
    // The decoder refuses the same tools, so nothing is ever decoded against a schema the model
    // was not shown.
    assert!(StreamDecoder::new(&[odd]).is_err());
}

// ── decoding ────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_plain_answer_has_no_calls_and_keeps_its_text() {
    let decoded = decode("I can't check the weather.", &tools()).unwrap();
    assert!(decoded.calls.is_empty());
    assert_eq!(decoded.text, "I can't check the weather.");
    assert_eq!(decode_calls("", &tools()).unwrap(), vec![]);
}

#[test]
fn text_before_between_and_after_calls_is_kept_apart_from_them() {
    let output = "Sure.\n<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Build\",\"body\":\"Green.\"}>>\n\
                  and\n<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>\nDone.";
    let decoded = decode(output, &tools()).unwrap();
    assert_eq!(decoded.text, "Sure.\n\nand\n\nDone.");
    let names: Vec<&str> = decoded.calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["send_email", "create_calendar_event"]);
    assert_eq!(arguments(&decoded.calls[1])["title"], "Retro");
}

#[test]
fn arguments_are_returned_exactly_as_written() {
    let written = "{ \"title\" : \"Caf\\u00e9 ☕\",\n  \"start\":\"2026-10-05T15:00:00+05:30\" }";
    let calls = decode_calls(
        &format!("<<call create_calendar_event {written}>>"),
        &tools(),
    )
    .unwrap();
    assert_eq!(calls[0].arguments, written);
}

#[test]
fn closing_markers_and_braces_inside_strings_need_no_escaping() {
    let output = r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"}>> <<call x {\"q\":1}>> {["}>>"#;
    let calls = decode_calls(output, &tools()).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(arguments(&calls[0])["subject"], "a >> b");
    assert_eq!(arguments(&calls[0])["body"], "}>> <<call x {\"q\":1}>> {[");
}

#[test]
fn rendered_calls_decode_back_to_themselves() {
    let calls = vec![
        ToolCall {
            name: "send_email".into(),
            arguments: json!({"to": ["sam@example.com"], "subject": "a >> b", "body": "The build is green."}).to_string(),
        },
        ToolCall {
            name: "create_calendar_event".into(),
            arguments: json!({"title": "Retro", "start": "2026-10-04T10:00:00+05:30", "duration_min": 30, "visibility": "private"}).to_string(),
        },
    ];
    let rendered = render_calls(&calls).unwrap();
    assert_eq!(decode_calls(&rendered, &tools()).unwrap(), calls);
}

// ── streaming ───────────────────────────────────────────────────────────────────────────────

#[test]
fn a_marker_split_across_chunks_decodes_like_the_whole() {
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let calls = stream(&chunks, &tools()).unwrap();
    assert_eq!(calls, decode_calls(&chunks.concat(), &tools()).unwrap());
    assert_eq!(arguments(&calls[0])["title"], "Retro");
}

#[test]
fn every_possible_split_point_gives_the_same_calls() {
    let output = "ok <<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"x >> y\",\"body\":\"é\"}>> then \
                  <<call create_calendar_event {\"title\":\"T\",\"start\":\"s\"}>> end";
    let whole = decode_calls(output, &tools()).unwrap();
    assert_eq!(whole.len(), 2);
    let chars: Vec<char> = output.chars().collect();
    for cut in 0..=chars.len() {
        let head: String = chars.iter().take(cut).collect();
        let tail: String = chars.iter().skip(cut).collect();
        assert_eq!(
            stream(&[&head, &tail], &tools()).unwrap(),
            whole,
            "split at {cut}"
        );
    }
    let one_by_one: Vec<String> = chars.iter().map(char::to_string).collect();
    let refs: Vec<&str> = one_by_one.iter().map(String::as_str).collect();
    assert_eq!(stream(&refs, &tools()).unwrap(), whole);
}

// ── fail closed ─────────────────────────────────────────────────────────────────────────────

#[test]
fn an_unknown_tool_is_an_error() {
    let error = decode_calls("<<call delete_everything {}>>", &[calendar()]).unwrap_err();
    assert_eq!(error, CompactError::UnknownTool("delete_everything".into()));
    assert_eq!(error.as_label(), "unknown_tool");
}

#[test]
fn a_tool_that_exists_but_was_not_offered_is_unknown() {
    let output = "<<call send_email {\"to\":[\"a@b.c\"],\"subject\":\"s\",\"body\":\"b\"}>>";
    assert!(decode_calls(output, &tools()).is_ok());
    assert_eq!(
        decode_calls(output, &[calendar()]).unwrap_err().as_label(),
        "unknown_tool"
    );
}

#[test]
fn tool_names_are_matched_exactly() {
    for name in [
        "Create_Calendar_Event",
        "create_calendar_even",
        "create_calendar_events",
    ] {
        let output = format!("<<call {name} {{\"title\":\"t\",\"start\":\"s\"}}>>");
        assert_eq!(
            decode_calls(&output, &tools()).unwrap_err().as_label(),
            "unknown_tool",
            "{name} was accepted"
        );
    }
}

#[test]
fn invalid_arguments_are_errors_never_repaired_calls() {
    for (args, why) in [
        (
            r#"{"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}"#,
            "missing required + bad enum",
        ),
        (r#"{"start":"s"}"#, "missing required"),
        (
            r#"{"title":"t","start":"s","visibility":"secret"}"#,
            "enum violation",
        ),
        (
            r#"{"title":"t","start":"s","duration_min":"30"}"#,
            "string for integer",
        ),
        (
            r#"{"title":"t","start":"s","attendees":"a@b.c"}"#,
            "string for array",
        ),
        (
            r#"{"title":"t","start":"s","colour":"red"}"#,
            "undeclared argument",
        ),
        (r#"{"title":"t","start":"s",}"#, "trailing comma"),
        (r#"{"title":"t" "start":"s"}"#, "broken JSON"),
        (r#"{title:"t",start:"s"}"#, "unquoted keys"),
    ] {
        let output = format!("<<call create_calendar_event {args}>>");
        let error = decode_calls(&output, &tools()).unwrap_err();
        assert_eq!(error.as_label(), "invalid_arguments", "{why}");
    }
}

#[test]
fn one_bad_call_fails_the_whole_output() {
    let output = "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>\n\
                  <<call create_calendar_event {\"title\":\"t\"}>>";
    assert!(decode(output, &tools()).is_err());
}

#[test]
fn malformed_markers_are_errors_not_text_and_not_calls() {
    for output in [
        "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}",
        "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>",
        "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"} >> trailing <<call",
        "<<call create_calendar_event title=t>>",
        "<<call create_calendar_event>>",
        "<<call>>",
        "<<call {\"title\":\"t\",\"start\":\"s\"}>>",
        "<<callcreate_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>",
        "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}}>>",
        "<<call create_calendar_event {\"title\":\"unterminated string}>>",
    ] {
        assert!(
            decode_calls(output, &tools()).is_err(),
            "accepted {output:?}"
        );
    }
}

#[test]
fn text_that_merely_resembles_a_marker_stays_text() {
    for output in [
        "a << b >> c",
        "<<cal",
        "<call x {}>",
        "use `x << 2` then <<",
        "<<Call x {}>>",
    ] {
        let decoded = decode(output, &tools()).unwrap();
        assert!(decoded.calls.is_empty());
        assert_eq!(decoded.text, output);
    }
}

// ── determinism ─────────────────────────────────────────────────────────────────────────────

#[test]
fn encoding_and_decoding_are_deterministic() {
    let output = "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>";
    let first = (
        encode_tools(&tools()).unwrap(),
        decode(output, &tools()).unwrap(),
    );
    for _ in 0..5 {
        assert_eq!(encode_tools(&tools()).unwrap(), first.0);
        assert_eq!(decode(output, &tools()).unwrap(), first.1);
    }
}

#[test]
fn multibyte_text_survives_everywhere() {
    let tool = ToolDef {
        name: "note".into(),
        description: Some("Écrire une note — 日本語".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {"текст": {"type": "string", "description": "naïve · café"}},
            "required": ["текст"]
        })),
    };
    let compact = encode_tools(std::slice::from_ref(&tool)).unwrap();
    assert_eq!(decode_tools(&compact).unwrap(), vec![tool.clone()]);

    let output = "voilà 👋 <<call note {\"текст\":\"日本語 >> ☕\"}>> fin";
    let chars: Vec<String> = output.chars().map(|c| c.to_string()).collect();
    let refs: Vec<&str> = chars.iter().map(String::as_str).collect();
    let calls = stream(&refs, std::slice::from_ref(&tool)).unwrap();
    assert_eq!(arguments(&calls[0])["текст"], "日本語 >> ☕");
}
