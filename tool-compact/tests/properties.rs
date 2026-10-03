//! Deterministic property tests (no `proptest`): exhaustive chunk-split invariance, seeded
//! garbage robustness, and render → decode round trips on generated arguments.

use nasiko_tool_compact::{
    DecodeError, Decoded, Event, StreamDecoder, ToolCall, ToolDef, decode, decode_calls,
    render_calls,
};
use serde_json::{Value, json};

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start": {"type": "string", "format": "date-time"},
                    "duration_min": {"type": "integer"},
                    "attendees": {"type": "array", "items": {"type": "string"}},
                    "visibility": {"type": "string", "enum": ["public", "private"]}
                },
                "required": ["title", "start"]
            })),
        },
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
    ]
}

/// Decode `chunks` through the streaming API, collapsing events the same way `decode` does.
fn stream(chunks: &[&str]) -> Result<Decoded, DecodeError> {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    let mut events = Vec::new();
    for c in chunks {
        events.extend(d.feed(c)?);
    }
    events.extend(d.finish()?);
    let mut out = Decoded::default();
    for e in events {
        match e {
            Event::Text(t) => out.text.push_str(&t),
            Event::Call(c) => out.calls.push(c),
        }
    }
    if !out.calls.is_empty()
        && out
            .text
            .lines()
            .all(|l| l.trim().is_empty() || l.trim().starts_with("```"))
    {
        out.text.clear();
    }
    Ok(out)
}

fn outcome(r: Result<Decoded, DecodeError>) -> Result<Decoded, &'static str> {
    r.map_err(|e| e.code())
}

fn corpus() -> Vec<String> {
    vec![
        r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30"}>>"#.into(),
        r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x"}>>"#.into(),
        r#"<<call delete_everything {}>>"#.into(),
        r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#.into(),
        "Done: <<call send_email {\"to\":[\"a@b.co\"],\"subject\":\"é ✓ {}\",\"body\":\"q \\\" >>\"}>> then\n<<call create_calendar_event {\"title\":\"R\",\"start\":\"2026-10-04T10:00:00Z\",\"duration_min\":30}>>!".into(),
        "plain text, riya@example.com, a << b >> c, <<caller".into(),
        "```\n<<call create_calendar_event {\"title\":\"t\",\"start\":\"2026-10-04T10:00:00Z\"}>>\n```".into(),
        "<<call create_calendar_event {\"title\":\"t\"".into(),
        "<<ca".into(),
    ]
}

/// All char-boundary split points of `s`.
fn boundaries(s: &str) -> Vec<usize> {
    s.char_indices().map(|(i, _)| i).skip(1).collect()
}

#[test]
fn every_two_way_split_decodes_identically() {
    for text in corpus() {
        let whole = outcome(decode(&text, &tools()));
        for i in boundaries(&text) {
            let (a, b) = (text.get(..i).unwrap(), text.get(i..).unwrap());
            assert_eq!(outcome(stream(&[a, b])), whole, "split at {i} of {text:?}");
        }
    }
}

#[test]
fn every_three_way_split_decodes_identically() {
    for text in corpus() {
        let whole = outcome(decode(&text, &tools()));
        let b = boundaries(&text);
        for (n, &i) in b.iter().enumerate() {
            for &j in b.iter().skip(n + 1) {
                let parts = [
                    text.get(..i).unwrap(),
                    text.get(i..j).unwrap(),
                    text.get(j..).unwrap(),
                ];
                assert_eq!(
                    outcome(stream(&parts)),
                    whole,
                    "split at {i},{j} of {text:?}"
                );
            }
        }
    }
}

#[test]
fn char_by_char_stream_equals_whole() {
    for text in corpus() {
        let chars: Vec<String> = text.chars().map(String::from).collect();
        let parts: Vec<&str> = chars.iter().map(String::as_str).collect();
        assert_eq!(
            outcome(stream(&parts)),
            outcome(decode(&text, &tools())),
            "{text:?}"
        );
    }
}

/// Seeded linear-congruential generator — deterministic, no dependency.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() as usize) % items.len()]
    }
}

#[test]
fn garbage_never_panics_and_is_text_or_error() {
    let atoms = [
        "<",
        ">",
        "<<",
        ">>",
        "{",
        "}",
        "[",
        "]",
        "\"",
        "\\",
        ":",
        ",",
        " ",
        "\n",
        "call",
        "<<call ",
        "create_calendar_event",
        "send_email",
        "title",
        "\"title\":\"x\"",
        "日本",
        "✓",
        "é",
        "1",
        "30.0",
        "null",
        "```",
    ];
    let mut rng = Lcg(0x5eed);
    for _ in 0..10_000 {
        let len = 1 + (rng.next() % 24) as usize;
        let text: String = (0..len).map(|_| rng.pick(&atoms)).collect();
        match decode(&text, &tools()) {
            Ok(d) => {
                for c in &d.calls {
                    // Anything that decodes must re-decode to itself (no half-validated calls).
                    let again =
                        decode_calls(&render_calls(std::slice::from_ref(c)), &tools()).unwrap();
                    assert_eq!(again.len(), 1);
                }
            }
            Err(e) => assert!(matches!(e.code(), "unknown_tool" | "invalid_arguments")),
        }
        // Streaming the same garbage in random chunks agrees with the one-shot result.
        let mut cuts: Vec<usize> = boundaries(&text)
            .into_iter()
            .filter(|_| rng.next().is_multiple_of(3))
            .collect();
        cuts.push(text.len());
        let mut parts = Vec::new();
        let mut prev = 0;
        for c in cuts {
            parts.push(text.get(prev..c).unwrap());
            prev = c;
        }
        assert_eq!(
            outcome(stream(&parts)),
            outcome(decode(&text, &tools())),
            "{text:?}"
        );
    }
}

#[test]
fn generated_calls_round_trip() {
    let mut rng = Lcg(7);
    let words = [
        "a",
        "Design review",
        "a >> b",
        "q\"uote",
        "back\\slash",
        "日本語",
        "{}",
        "x y z",
    ];
    for _ in 0..2_000 {
        let mut args = serde_json::Map::new();
        args.insert("title".into(), json!(rng.pick(&words)));
        args.insert("start".into(), json!("2026-10-05T15:00:00+05:30"));
        if rng.next().is_multiple_of(2) {
            args.insert("duration_min".into(), json!(rng.next() % 600));
        }
        if rng.next().is_multiple_of(2) {
            args.insert(
                "attendees".into(),
                json!([rng.pick(&words), rng.pick(&words)]),
            );
        }
        if rng.next().is_multiple_of(2) {
            args.insert("visibility".into(), json!(rng.pick(&["public", "private"])));
        }
        let call = ToolCall {
            name: "create_calendar_event".into(),
            arguments: Value::Object(args).to_string(),
        };
        let calls = vec![call.clone(), call];
        let back = decode_calls(&render_calls(&calls), &tools()).unwrap();
        assert_eq!(back, calls);
    }
}
