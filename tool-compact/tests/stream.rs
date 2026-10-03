mod common;

use common::*;
use nasiko_tool_compact::{StreamDecoder, ToolCall, ToolDef, decode_calls};
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    vec![calendar(), email()]
}

fn run_chunks(chunks: &[&str]) -> Result<Vec<ToolCall>, nasiko_tool_compact::Error> {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    let mut out = Vec::new();
    for c in chunks {
        out.extend(d.push(c)?);
    }
    d.finish()?;
    Ok(out)
}

const TWO_CALLS: &str = concat!(
    "Okay — नमस्ते. ",
    r#"<<call send_email {"to":["sam@example.com"],"subject":"a >> b","body":"x\\"}>>"#,
    " then ",
    r#"<<call create_calendar_event {"title":"Retro \"1\"","start":"2026-10-04T10:00:00+05:30","duration_min":30}>>"#,
    " bye",
);

/// Every way of cutting `s` into 2 and 3 pieces at char boundaries.
fn splits(s: &str) -> Vec<Vec<String>> {
    let cuts: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
    let mut out = vec![];
    for (a, &i) in cuts.iter().enumerate() {
        out.push(vec![s[..i].to_string(), s[i..].to_string()]);
        for &j in &cuts[a..] {
            out.push(vec![
                s[..i].to_string(),
                s[i..j].to_string(),
                s[j..].to_string(),
            ]);
        }
    }
    out
}

#[test]
fn issue_example_chunks_decode_like_the_whole_string() {
    let chunks = [
        "<<cal",
        "l create_calendar_event {",
        "\"title\":\"Meeting\",\"start\":\"2026-10-05T15:00:00Z\"",
        "}>>",
    ];
    let streamed = run_chunks(&chunks).unwrap();
    let whole = decode_calls(&chunks.concat(), &tools()).unwrap();
    assert_eq!(streamed, whole);
    assert_eq!(streamed.len(), 1);
}

#[test]
fn split_marker_open_tool_name_json_and_close() {
    let text = r#"<<call create_calendar_event {"title":"Ret","start":"2026-10-04T10:00:00Z"}>>"#;
    let want = decode_calls(text, &tools()).unwrap();
    for cut in [1, 2, 5, 6, 7, 12, 30, 40, text.len() - 2, text.len() - 1] {
        let (a, b) = text.split_at(cut);
        assert_eq!(run_chunks(&[a, b]).unwrap(), want, "cut at {cut}");
    }
}

#[test]
fn every_two_and_three_way_split_matches_the_batch_result() {
    let want = decode_calls(TWO_CALLS, &tools()).unwrap();
    assert_eq!(want.len(), 2);
    for parts in splits(TWO_CALLS) {
        let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
        assert_eq!(run_chunks(&refs).unwrap(), want, "{parts:?}");
    }
}

#[test]
fn char_by_char_matches_the_batch_result() {
    let want = decode_calls(TWO_CALLS, &tools()).unwrap();
    let cs: Vec<String> = TWO_CALLS.chars().map(String::from).collect();
    let refs: Vec<&str> = cs.iter().map(String::as_str).collect();
    assert_eq!(run_chunks(&refs).unwrap(), want);
}

#[test]
fn errors_are_the_same_however_the_text_is_chunked() {
    for text in [
        r#"<<call delete_everything {}>>"#,
        r#"<<call send_email {"to":[]}>>"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"}>"#,
        r#"<<call send_email {"to":[],"subject":"s","body":"b"} x>>"#,
        "<<call send_email",
    ] {
        let want = decode_calls(text, &tools()).unwrap_err();
        for parts in splits(text) {
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            assert_eq!(run_chunks(&refs).unwrap_err(), want, "{text} {parts:?}");
        }
    }
}

#[test]
fn gt_gt_inside_a_string_split_across_chunks() {
    let chunks = [
        r#"<<call send_email {"to":[],"subject":"a >"#,
        r#"> b","body":"x"}>"#,
        ">",
    ];
    let calls = run_chunks(&chunks).unwrap();
    assert_eq!(args(&calls[0])["subject"], "a >> b");
}

#[test]
fn escape_sequence_split_at_a_chunk_boundary() {
    // `\` at the end of one chunk, the escaped quote at the start of the next.
    let chunks = [
        r#"<<call send_email {"to":[],"subject":"q\"#,
        r#"" >> z","body":"b"}>>"#,
    ];
    let calls = run_chunks(&chunks).unwrap();
    assert_eq!(args(&calls[0])["subject"], "q\" >> z");
}

#[test]
fn multiple_streaming_calls_are_emitted_as_each_one_completes() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(
        d.push("hello <<call send_email {\"to\":[],\"subject\":\"s\",")
            .unwrap()
            .is_empty()
    );
    let first = d.push("\"body\":\"b\"}>>").unwrap();
    assert_eq!(first.len(), 1, "call must be emitted as soon as it closes");
    let second = d
        .push(r#" and <<call create_calendar_event {"title":"t","start":"2026-10-04T10:00:00Z"}>> done"#)
        .unwrap();
    assert_eq!(second.len(), 1);
    d.finish().unwrap();
}

#[test]
fn partial_prefix_at_the_end_is_plain_text_not_a_call() {
    assert!(run_chunks(&["abc <<ca"]).unwrap().is_empty());
    assert!(
        run_chunks(&["abc <", "<c", "all"]).is_err(),
        "`<<call` then EOF is an unterminated marker"
    );
}

#[test]
fn unfinished_marker_fails_at_finish_not_silently() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    assert!(
        d.push(r#"<<call send_email {"to":[],"subject":"s""#)
            .unwrap()
            .is_empty()
    );
    assert_eq!(d.finish().unwrap_err().code(), "malformed_call");
}

#[test]
fn decoder_stays_failed_after_an_error() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    let e = d.push("<<call nope {}>>").unwrap_err();
    assert_eq!(e.code(), "unknown_tool");
    // Valid input after the failure must not resurrect the stream.
    assert_eq!(
        d.push(r#"<<call send_email {"to":[],"subject":"s","body":"b"}>>"#)
            .unwrap_err(),
        e
    );
    assert_eq!(d.finish().unwrap_err(), e);
}

#[test]
fn oversized_marker_is_rejected_instead_of_buffered_forever() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push("<<call send_email {\"to\":[],\"subject\":\"")
        .unwrap();
    let chunk = "a".repeat(64 * 1024);
    let mut failed = false;
    for _ in 0..32 {
        if d.push(&chunk).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed, "a marker that never closes must hit the size cap");
}

#[test]
fn eval_style_decoder_cases() {
    // dc-002 from the public sample set.
    let chunks = [
        "<<ca",
        "ll create_calendar_event {\"title\":\"Ret",
        "ro\",\"start\":\"2026-10-04T10:00:00+05:30\"}>",
        ">",
    ];
    let calls = run_chunks(&chunks).unwrap();
    assert_eq!(
        args(&calls[0]),
        json!({"title":"Retro","start":"2026-10-04T10:00:00+05:30"})
    );
}

// ── plain text outside markers (so a router can return the assistant's prose) ───────────

/// What `TWO_CALLS` says once its two markers are removed.
const TWO_CALLS_TEXT: &str = "Okay — नमस्ते.  then  bye";

fn run_text(chunks: &[&str]) -> (Vec<ToolCall>, String) {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    let (mut calls, mut text) = (Vec::new(), String::new());
    for c in chunks {
        calls.extend(d.push(c).unwrap());
        text.push_str(&d.take_text());
    }
    text.push_str(&d.finish().unwrap());
    (calls, text)
}

#[test]
fn text_outside_markers_is_returned_without_the_markers() {
    let (calls, text) = run_text(&[TWO_CALLS]);
    assert_eq!(calls.len(), 2);
    assert_eq!(text, TWO_CALLS_TEXT);
}

#[test]
fn text_is_identical_for_every_chunking() {
    for parts in splits(TWO_CALLS) {
        let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
        let (calls, text) = run_text(&refs);
        assert_eq!(calls.len(), 2, "{parts:?}");
        assert_eq!(text, TWO_CALLS_TEXT, "{parts:?}");
    }
}

#[test]
fn a_possible_marker_start_is_held_back_until_it_is_resolved() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push("hi <<ca").unwrap();
    assert_eq!(d.take_text(), "hi ", "`<<ca` might still become a marker");
    d.push("t x").unwrap();
    assert_eq!(
        d.take_text(),
        "<<cat x",
        "…and is plain text once it cannot"
    );
    assert_eq!(d.take_text(), "", "text is handed out once");
}

#[test]
fn trailing_partial_marker_start_comes_back_from_finish() {
    let mut d = StreamDecoder::new(&tools()).unwrap();
    d.push("abc <<ca").unwrap();
    assert_eq!(d.take_text(), "abc ");
    assert_eq!(d.finish().unwrap(), "<<ca");
}

#[test]
fn marker_lookalikes_stay_in_the_text() {
    let (calls, text) = run_text(&["a << b >> c <<callous>> <<call>> end"]);
    assert!(calls.is_empty());
    assert_eq!(text, "a << b >> c <<callous>> <<call>> end");
}
