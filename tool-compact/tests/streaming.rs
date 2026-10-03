use nasiko_tool_compact::{decode_response, Limits, StreamDecoder, ToolDef};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    ToolDef::new(
        "create_calendar_event",
        Some("Create an event in the user's calendar.".to_string()),
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "start": { "type": "string", "format": "date-time" },
                "attendees": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["title", "start"]
        }),
    )
}

#[test]
fn split_every_byte_offset() {
    let tools = vec![calendar_tool()];
    let golden = "Here is the plan: <<call create_calendar_event {\"title\":\"Meeting\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> and done!";
    let baseline = decode_response(golden, &tools).expect("baseline decode");

    let bytes = golden.as_bytes();
    for split in 0..=bytes.len() {
        let (c1, c2) = bytes.split_at(split);
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        decoder.push(c1).unwrap();
        decoder.push(c2).unwrap();
        let res = decoder.finish().expect(&format!("failed at split offset {}", split));
        assert_eq!(res, baseline, "mismatch at split offset {}", split);
    }
}

#[test]
fn one_byte_chunks() {
    let tools = vec![calendar_tool()];
    let golden = "Leading text <<call create_calendar_event {\"title\":\"1-byte chunks\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> Trailing text.";
    let baseline = decode_response(golden, &tools).expect("baseline");

    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    for &b in golden.as_bytes() {
        decoder.push(&[b]).unwrap();
    }
    let res = decoder.finish().expect("finish 1-byte chunks");
    assert_eq!(res, baseline);
}

#[test]
fn random_chunk_partitions() {
    let tools = vec![calendar_tool()];
    let golden = "Prefix <<call create_calendar_event {\"title\":\"Chunk partition test\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> Suffix.";
    let baseline = decode_response(golden, &tools).unwrap();

    let bytes = golden.as_bytes();
    let chunk_sizes = [1, 2, 3, 5, 7, 11, 13, 20, 50];
    for &cs in &chunk_sizes {
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        for chunk in bytes.chunks(cs) {
            decoder.push(chunk).unwrap();
        }
        let res = decoder.finish().unwrap();
        assert_eq!(res, baseline);
    }
}

#[test]
fn split_inside_utf8_character() {
    let tools = vec![calendar_tool()];
    // "Design review 🎯" -> 🎯 is 4 UTF-8 bytes: [0xF0, 0x9F, 0x8E, 0xAF]
    let golden = "Prefix <<call create_calendar_event {\"title\":\"Design review 🎯\",\"start\":\"2026-10-05T10:00:00+05:30\"}>> suffix";
    let bytes = golden.as_bytes();
    let target_idx = golden.find("🎯").unwrap();

    // Split right inside the 4-byte sequence: at byte 1, 2, 3
    for offset in 1..4 {
        let split_pos = target_idx + offset;
        let (c1, c2) = bytes.split_at(split_pos);
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        decoder.push(c1).unwrap();
        decoder.push(c2).unwrap();
        let res = decoder.finish().unwrap();
        assert_eq!(res.calls[0].arguments["title"], "Design review 🎯");
    }
}

#[test]
fn split_inside_unicode_escape() {
    let tools = vec![calendar_tool()];
    // "\\u0041" -> 'A'
    let golden = r#"<<call create_calendar_event {"title":"Prefix\u0041Suffix","start":"2026-10-05T10:00:00+05:30"}>>"#;
    let target_idx = golden.find(r#"\u0041"#).unwrap();
    let bytes = golden.as_bytes();

    for offset in 1..6 {
        let (c1, c2) = bytes.split_at(target_idx + offset);
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        decoder.push(c1).unwrap();
        decoder.push(c2).unwrap();
        let res = decoder.finish().unwrap();
        assert_eq!(res.calls[0].arguments["title"], "PrefixASuffix");
    }
}

#[test]
fn split_inside_opening_marker() {
    let tools = vec![calendar_tool()];
    let golden = "Hello <<call create_calendar_event {\"title\":\"Opening split\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>";
    let marker_idx = golden.find("<<call").unwrap();
    let bytes = golden.as_bytes();

    for offset in 1..6 {
        let (c1, c2) = bytes.split_at(marker_idx + offset);
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        decoder.push(c1).unwrap();
        decoder.push(c2).unwrap();
        let res = decoder.finish().unwrap();
        assert_eq!(res.calls.len(), 1);
    }
}

#[test]
fn split_between_terminator_bytes() {
    let tools = vec![calendar_tool()];
    let golden = "<<call create_calendar_event {\"title\":\"Terminator split\",\"start\":\"2026-10-05T10:00:00+05:30\"}>>";
    let end_idx = golden.len() - 1; // split between the two '>'
    let (c1, c2) = golden.as_bytes().split_at(end_idx);

    let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
    decoder.push(c1).unwrap();
    decoder.push(c2).unwrap();
    let res = decoder.finish().unwrap();
    assert_eq!(res.calls.len(), 1);
}
