use nasiko_tool_compact::*;
use serde_json::{Value, json};

fn cal() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".into()),
        Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]},
                "meta": {"type": "object", "properties": {"room": {"type": "string"}}, "additionalProperties": false}
            },
            "required": ["title", "start"]
        })),
    )
}

fn strip_desc(v: &Value) -> Value {
    let mut v = v.clone();
    fn s(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("description");
                for (_, x) in m.iter_mut() {
                    s(x);
                }
            }
            Value::Array(a) => {
                for x in a {
                    s(x);
                }
            }
            _ => {}
        }
    }
    s(&mut v);
    v
}

#[test]
fn test_probe_regression_suite() {
    let tools = vec![cal()];
    let enc = encode_tools(&tools).unwrap();

    // A3: schema round trip, ignoring descriptions
    let dec = decode_tools(&enc).unwrap();
    assert_eq!(
        strip_desc(dec[0].parameters.as_ref().unwrap()),
        strip_desc(tools[0].parameters.as_ref().unwrap())
    );

    let ok = r#"{"title":"Retro","start":"2026-10-04T10:00:00+05:30"}"#;

    // A2: non-streaming decode_calls fails when truncated (no >>)
    assert!(decode_calls(&format!("<<call create_calendar_event {ok}"), &tools).is_err());

    // A2: accept newline and tab after marker
    assert!(decode_calls(&format!("<<call\ncreate_calendar_event {ok}>>"), &tools).is_ok());
    assert!(decode_calls(&format!("<<call\tcreate_calendar_event {ok}>>"), &tools).is_ok());

    // A4: bad datetime rejected
    assert!(
        decode_calls(
            r#"<<call create_calendar_event {"title":"x","start":"2026-13-45T99:99:99"}>>"#,
            &tools
        )
        .is_err()
    );

    // A4: missing offset rejected
    assert!(
        decode_calls(
            r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00"}>>"#,
            &tools
        )
        .is_err()
    );

    // A4: nested additionalProperties: false enforced
    assert!(decode_calls(
        r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00+05:30","meta":{"room":"a","evil":1}}>>"#,
        &tools
    )
    .is_err());

    // Enum validation
    assert!(decode_calls(
        r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00+05:30","visibility":"secret"}>>"#,
        &tools
    )
    .is_err());

    // Missing required
    assert!(
        decode_calls(
            r#"<<call create_calendar_event {"start":"2026-10-04T10:00:00+05:30"}>>"#,
            &tools
        )
        .is_err()
    );

    // Float for int accepted if integer value (e.g. 30.0)
    assert!(decode_calls(
        r#"<<call create_calendar_event {"title":"x","start":"2026-10-04T10:00:00+05:30","duration_min":30.0}>>"#,
        &tools
    )
    .is_ok());

    // Stream text conservation including "x <<" with flush
    for input in ["Answer: a < b", "x <<", "Use << to shift"] {
        let mut d = StreamDecoder::new(tools.clone());
        let mut out = String::new();
        for ch in input.chars() {
            for ev in d.push(&ch.to_string()) {
                if let StreamEvent::Text(t) = ev {
                    out.push_str(&t);
                }
            }
        }
        for ev in d.flush() {
            if let StreamEvent::Text(t) = ev {
                out.push_str(&t);
            }
        }
        let fin = d.finish();
        assert_eq!(out, input);
        assert!(fin.is_ok());
    }

    // Call emitted before error
    let mut d = StreamDecoder::new(tools.clone());
    let evs = d.push(&format!(
        "<<call create_calendar_event {ok}>> <<call create_calendar_event {{\"title\":1}}>>"
    ));
    let n_calls = evs
        .iter()
        .filter(|e| matches!(e, StreamEvent::Call(_)))
        .count();
    assert_eq!(n_calls, 1);
    assert!(d.finish().is_err());

    // Split with markers in string across chunk boundaries
    let s = r#"pre <<call create_calendar_event {"title":"a >> b }} <<call x","start":"2026-10-04T10:00:00+05:30"}>> post"#;
    for i in 1..s.len() {
        if !s.is_char_boundary(i) {
            continue;
        }
        let mut d = StreamDecoder::new(tools.clone());
        let _ = d.push(&s[..i]);
        let _ = d.push(&s[i..]);
        let c = d.finish().expect("split decode must succeed");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].arguments["title"], "a >> b }} <<call x");
    }

    // Space before close
    assert_eq!(
        decode_calls(&format!("<<call create_calendar_event {ok} >>"), &tools)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn test_probe_eval_decoder_cases() {
    let tools = vec![
        cal(),
        ToolDef::new(
            "send_email",
            Some("Send an email.".into()),
            Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string", "format": "email"}, "description": "Recipient emails"},
                    "subject": {"type": "string"},
                    "body": {"type": "string"}
                },
                "required": ["to", "subject", "body"]
            })),
        ),
    ];

    // dc-002: marker split across chunks
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<ca");
    d.push("ll create_calendar_event {\"title\":\"Ret");
    d.push("ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>");
    d.push(">");
    let res = d.finish().unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].name, "create_calendar_event");
    assert_eq!(res[0].arguments["title"], "Retro");

    // dc-unknown
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<call delete_everything {}>>");
    assert!(matches!(d.finish(), Err(CompactError::UnknownTool(_))));

    // dc-enum
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<call create_calendar_event {\"title\":\"x\",\"start\":\"2026-10-04T10:00:00+05:30\",\"visibility\":\"secret\"}>>");
    assert!(matches!(
        d.finish(),
        Err(CompactError::InvalidArguments { .. })
    ));

    // dc-missing
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<call create_calendar_event {\"start\":\"2026-10-04T10:00:00+05:30\"}>>");
    assert!(matches!(
        d.finish(),
        Err(CompactError::InvalidArguments { .. })
    ));

    // dc-escape
    let mut d = StreamDecoder::new(tools.clone());
    d.push("Sure. <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"s >");
    d.push("> t\",\"body\":\"x }} <<call y {}>> z\"}>> Done.");
    let res = d.finish().unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].name, "send_email");
    assert_eq!(res[0].arguments["subject"], "s >> t");
    assert_eq!(res[0].arguments["body"], "x }} <<call y {}>> z");

    // dc-truncated
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<call create_calendar_event {\"title\":\"x\"");
    assert!(matches!(
        d.finish(),
        Err(CompactError::InvalidArguments { .. })
    ));

    // dc-badjson
    let mut d = StreamDecoder::new(tools.clone());
    d.push("<<call create_calendar_event {\"title\":\"x\",\"start\":}>>");
    assert!(matches!(
        d.finish(),
        Err(CompactError::InvalidArguments { .. })
    ));

    // dc-plain
    let mut d = StreamDecoder::new(tools.clone());
    d.push("It is sunny ");
    d.push("today.");
    let res = d.finish().unwrap();
    assert_eq!(res.len(), 0);

    // dc-multi
    let mut d = StreamDecoder::new(tools.clone());
    d.push("A <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"s\",\"body\":\"b\"}>> B <<call create_calendar_event {\"title\":\"t\",\"start\":\"2026-10-04T10:00:00+05:30\"}>> C");
    let res = d.finish().unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].name, "send_email");
    assert_eq!(res[1].name, "create_calendar_event");
}
