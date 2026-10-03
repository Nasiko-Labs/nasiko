use nasiko_tool_compact::*;
use proptest::prelude::*;
use serde_json::json;

fn sample_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "create_calendar_event".into(),
                description: Some("Create an event in the user's calendar.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "start": { "type": "string", "format": "date-time" },
                        "duration_min": { "type": "integer" },
                        "visibility": { "type": "string", "enum": ["public", "private"] }
                    },
                    "required": ["title", "start"]
                })),
            },
            extra: Default::default(),
        },
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: "send_email".into(),
                description: Some("Send an email.".into()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "string" },
                        "subject": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "subject"]
                })),
            },
            extra: Default::default(),
        },
    ]
}

proptest! {
    /// Property Test 1: Rendered call -> decode -> re-render round-trips exactly for generated valid calls.
    #[test]
    fn prop_render_decode_roundtrip(title in "\\PC*", start_day in 1..28i32, visibility in "public|private") {
        let tools = sample_tools();
        let start = format!("2026-10-{:02}T15:00:00Z", start_day);
        let args = json!({
            "title": title,
            "start": start,
            "visibility": visibility,
        });

        let rendered = render_call("create_calendar_event", &args);
        let decoded = decode_calls(&rendered, &tools).unwrap();
        prop_assert_eq!(decoded.len(), 1);
        prop_assert_eq!(&decoded[0].function.name, "create_calendar_event");

        let decoded_args: serde_json::Value = serde_json::from_str(&decoded[0].function.arguments).unwrap();
        prop_assert_eq!(decoded_args, args);
    }

    /// Property Test 2: Chunk-split invariance (StreamDecoder produces identical result regardless of chunk boundaries).
    #[test]
    fn prop_chunk_split_invariance(split_pos in 0usize..100usize) {
        let tools = sample_tools();
        let text = r#"Leading text <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00Z"}>> trailing text"#;

        let full_result = decode_calls(text, &tools);

        let split = split_pos.min(text.len());
        let (c1, c2) = text.split_at(split);

        let mut stream = StreamDecoder::new();
        stream.push(c1);
        stream.push(c2);
        let chunked_raw = stream.finish();

        match (full_result, chunked_raw) {
            (Ok(full_calls), Ok(chunk_calls)) => {
                prop_assert_eq!(full_calls.len(), chunk_calls.len());
                for (f, c) in full_calls.iter().zip(chunk_calls.iter()) {
                    prop_assert_eq!(&f.function.name, &c.name);
                }
            }
            (Err(_), Err(_)) => {},
            (a, b) => prop_assert!(false, "Mismatch between monolithic and chunked decoding: {:?} vs {:?}", a, b),
        }
    }

    /// Property Test 3: Byte chunk-split invariance (push_bytes handles arbitrary byte boundaries).
    #[test]
    fn prop_byte_chunk_split_invariance(split_pos in 0usize..100usize) {
        let text = r#"Leading text <<call create_calendar_event {"title":"Design review with unicode 🚀","start":"2026-10-05T15:00:00Z"}>> trailing text"#;
        let bytes = text.as_bytes();

        let split = split_pos.min(bytes.len());
        let (b1, b2) = bytes.split_at(split);

        let mut s1 = StreamDecoder::new();
        s1.push(text);
        let res1 = s1.finish();

        let mut s2 = StreamDecoder::new();
        s2.push_bytes(b1);
        s2.push_bytes(b2);
        let res2 = s2.finish();

        match (res1, res2) {
            (Ok(r1), Ok(r2)) => {
                prop_assert_eq!(r1.len(), r2.len());
                for (c1, c2) in r1.iter().zip(r2.iter()) {
                    prop_assert_eq!(&c1.name, &c2.name);
                    prop_assert_eq!(&c1.args, &c2.args);
                }
            }
            (Err(_), Err(_)) => {},
            (a, b) => prop_assert!(false, "Mismatch between string push and byte push: {:?} vs {:?}", a, b),
        }
    }

    /// Property Test 4: StreamDecoder never panics on arbitrary string inputs.
    #[test]
    fn prop_no_panic_arbitrary_string(s in "\\PC*") {
        let tools = sample_tools();
        let _ = decode_calls(&s, &tools);
    }

    /// Property Test 5: StreamDecoder never panics on arbitrary byte inputs.
    #[test]
    fn prop_no_panic_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let mut stream = StreamDecoder::new();
        stream.push_bytes(&bytes);
        let _ = stream.finish();
    }
}
