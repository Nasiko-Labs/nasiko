//! Chunking never changes the result: every split of a reply decodes exactly like the whole.

mod common;

use common::{Lcg, tool};
use nasiko_tool_compact::{Decoded, StreamDecoder, ToolCompactError, ToolDef, decode_calls};
use serde_json::json;

fn catalog() -> Vec<ToolDef> {
    vec![
        tool(
            "create_calendar_event",
            json!({"type": "object", "properties": {
            "title": {"type": "string"}, "start": {"type": "string"}, "n": {"type": "integer"}
        }, "required": ["title", "start"]}),
        ),
        tool(
            "send_email",
            json!({"type": "object", "properties": {
            "to": {"type": "array", "items": {"type": "string"}}, "subject": {"type": "string"}, "body": {"type": "string"}
        }, "required": ["to", "subject", "body"]}),
        ),
        tool("noop", json!({"type": "object", "properties": {}})),
    ]
}

const SAMPLES: &[&str] = &[
    "<<call create_calendar_event {\"title\":\"Retro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>",
    "<<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"a >> b\",\"body\":\"x\"}>>",
    "Sure.\n<<call send_email {\"to\":[\"s\"],\"subject\":\"\\\"q\\\" \\\\ }>>\",\"body\":\"日本語 🎉\"}>>\nThen:\n<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"n\":3}>>\nDone 🎉",
    "plain prose with << and <<ca and <<callx inside, and >> too",
    "<<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>",
    "<<ca<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>",
    "<<call unknown_tool {}>>",
    "<<call noop>>",
    "<<call noop()>>",
    "ok <<call noop ( ) >> done",
    "<<call noop (x)>>",
    "<<call create_calendar_event()>>",
    "<<call create_calendar_event {\"start\":\"s\"}>>",
    "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\",\"n\":\"3\"}>>",
    "<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>",
    "<<call create_calendar_event {\"a\":1,\"a\":2}>>",
    "éé<<call send_email {\"to\":[\"é\"],\"subject\":\"é\",\"body\":\"é\"}>>éé",
];

fn one_shot(text: &str) -> Result<Decoded, ToolCompactError> {
    decode_calls(text, &catalog())
}

fn chunked(parts: &[&str]) -> Result<Decoded, ToolCompactError> {
    let mut d = StreamDecoder::new(&catalog())?;
    for p in parts {
        d.push(p)?;
    }
    d.finish()
}

#[test]
fn every_two_way_split_matches_one_shot() {
    for text in SAMPLES {
        let expected = one_shot(text);
        for i in 0..=text.len() {
            if !text.is_char_boundary(i) {
                continue;
            }
            let got = chunked(&[&text[..i], &text[i..]]);
            assert_eq!(got, expected, "{text:?} split at {i}");
        }
    }
}

#[test]
fn every_three_way_split_matches_one_shot_for_short_replies() {
    for text in SAMPLES.iter().filter(|t| t.len() <= 60) {
        let expected = one_shot(text);
        for i in 0..=text.len() {
            if !text.is_char_boundary(i) {
                continue;
            }
            for j in i..=text.len() {
                if !text.is_char_boundary(j) {
                    continue;
                }
                let got = chunked(&[&text[..i], &text[i..j], &text[j..]]);
                assert_eq!(got, expected, "{text:?} split at {i},{j}");
            }
        }
    }
}

#[test]
fn seeded_random_partitions_match_one_shot() {
    let mut rng = Lcg::new(99);
    for text in SAMPLES {
        let expected = one_shot(text);
        for _ in 0..60 {
            let mut parts: Vec<&str> = Vec::new();
            let mut start = 0;
            while start < text.len() {
                let mut end = start + 1 + rng.below(12) as usize;
                end = end.min(text.len());
                while !text.is_char_boundary(end) {
                    end += 1;
                }
                parts.push(&text[start..end]);
                start = end;
            }
            let got = chunked(&parts);
            assert_eq!(got, expected, "{text:?} parts {parts:?}");
        }
    }
}

#[test]
fn empty_chunks_and_single_character_chunks_are_fine() {
    for text in SAMPLES {
        let expected = one_shot(text);
        let mut d = StreamDecoder::new(&catalog()).unwrap();
        let mut result = Ok(());
        d.push("").unwrap();
        for c in text.chars() {
            let s = c.to_string();
            if let Err(e) = d.push(&s) {
                result = Err(e);
                break;
            }
            d.push("").unwrap();
        }
        let got = match result {
            Ok(()) => d.finish(),
            Err(e) => Err(e),
        };
        assert_eq!(got, expected, "{text:?}");
    }
}

#[test]
fn finish_without_calls_returns_all_text_including_a_dangling_marker_prefix() {
    let mut d = StreamDecoder::new(&catalog()).unwrap();
    d.push("hello <<ca").unwrap();
    assert_eq!(d.finish().unwrap().content, "hello <<ca");
    let mut d = StreamDecoder::new(&catalog()).unwrap();
    d.push("hello <<call").unwrap();
    assert_eq!(d.finish().unwrap().content, "hello <<call");
    let mut d = StreamDecoder::new(&catalog()).unwrap();
    d.push("hello <<call ").unwrap();
    assert_eq!(d.finish().unwrap_err().kind(), "incomplete_call");
}

#[test]
fn a_failed_decoder_stays_failed() {
    let mut d = StreamDecoder::new(&catalog()).unwrap();
    let first = d.push("<<call unknown_tool {}>>").unwrap_err();
    assert_eq!(first.kind(), "unknown_tool");
    assert_eq!(
        d.push("<<call create_calendar_event {\"title\":\"t\",\"start\":\"s\"}>>")
            .unwrap_err(),
        first
    );
    assert_eq!(d.push("plain").unwrap_err(), first);
    assert_eq!(d.finish().unwrap_err(), first);
}
