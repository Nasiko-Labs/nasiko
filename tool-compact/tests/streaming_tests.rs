use nasiko_tool_compact::{StreamDecoder, ToolDef};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    serde_json::from_value(json!({
        "type": "function",
        "function": {
            "name": "create_calendar_event",
            "description": "Create an event in the user's calendar.",
            "parameters": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "start": { "type": "string", "format": "date-time" },
                    "duration_min": { "type": "integer" }
                },
                "required": ["title", "start"]
            }
        }
    }))
    .unwrap()
}

#[test]
fn test_dc_002_marker_split_across_chunks() {
    let tools = vec![calendar_tool()];
    let chunks = vec![
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];

    let mut decoder = StreamDecoder::new(tools);
    let mut total_emitted = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        let emitted = decoder.push_chunk(chunk);
        if i < 3 {
            assert!(
                emitted.is_empty(),
                "Chunk {i} should not emit premature call"
            );
        } else {
            assert_eq!(emitted.len(), 1, "Chunk 3 should emit the completed call");
        }
        total_emitted.extend(emitted);
    }

    let final_calls = decoder.finish().expect("stream should finish cleanly");
    assert_eq!(final_calls.len(), 1);
    assert_eq!(final_calls[0].function.name, "create_calendar_event");

    let args: serde_json::Value = serde_json::from_str(&final_calls[0].function.arguments).unwrap();
    assert_eq!(args["title"], "Retro");
    assert_eq!(args["start"], "2026-10-04T10:00:00+05:30");
}

#[test]
fn test_single_byte_chunk_stream() {
    let tools = vec![calendar_tool()];
    let full_text = "<<call create_calendar_event {\"title\":\"Single byte test\",\"start\":\"2026-10-04T10:00:00Z\"}>>";

    let mut decoder = StreamDecoder::new(tools);
    let mut emitted = Vec::new();

    for ch in full_text.chars() {
        let chunk = ch.to_string();
        let calls = decoder.push_chunk(&chunk);
        emitted.extend(calls);
    }

    let final_calls = decoder.finish().expect("should finish cleanly");
    assert_eq!(final_calls.len(), 1);
    assert_eq!(emitted.len(), 1);
    assert_eq!(final_calls[0].function.name, "create_calendar_event");
}

#[test]
fn test_streaming_conversational_text_isolation() {
    let tools = vec![calendar_tool()];
    let mut decoder = StreamDecoder::new(tools);

    // Chunk 1: purely conversational text
    let (text1, calls1) = decoder.push_chunk_with_text("Hello! I can help you book an event. ");
    assert_eq!(
        text1.as_deref(),
        Some("Hello! I can help you book an event. ")
    );
    assert!(calls1.is_empty());

    // Chunk 2: text preceding call + start of call
    let (text2, calls2) =
        decoder.push_chunk_with_text("Scheduling now:\n<<call create_calendar_event ");
    assert_eq!(text2.as_deref(), Some("Scheduling now:\n"));
    assert!(calls2.is_empty());

    // Chunk 3: call payload and closing marker
    let (text3, calls3) = decoder
        .push_chunk_with_text("{\"title\":\"Standup\",\"start\":\"2026-10-05T09:00:00Z\"}>>");
    assert!(text3.is_none(), "call marker must not be emitted as text");
    assert_eq!(calls3.len(), 1);
    assert_eq!(calls3[0].function.name, "create_calendar_event");

    // Chunk 4: trailing conversational text
    let (text4, calls4) = decoder.push_chunk_with_text("\nAll booked for you!");
    assert_eq!(text4.as_deref(), Some("\nAll booked for you!"));
    assert!(calls4.is_empty());

    let (rem, final_calls) = decoder.finish_with_text().expect("clean finish");
    assert!(rem.is_none());
    assert_eq!(final_calls.len(), 1);
}
