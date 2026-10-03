//! Streaming properties. We state exactly what is tested:
//!
//! * every two-chunk split of fixed texts (at every char boundary),
//! * randomized multi-chunk partitions (proptest),
//! * incomplete final input, exactly-once emission, and no panic on arbitrary input.
//!
//! This is not a proof over all possible partitions.

use nasiko_tool_compact::{DecodeError, StreamDecoder, ToolCall, ToolDef, decode_calls};
use proptest::prelude::*;
use serde_json::json;

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send_email".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                    "prio": {"type": "string", "enum": ["low", "high"]}
                },
                "required": ["to", "subject", "body"]
            })),
        },
        ToolDef {
            name: "ping".into(),
            description: None,
            parameters: None,
        },
    ]
}

/// Valid and invalid texts covering escapes, `>>` in strings, unicode, prose and errors.
fn corpus() -> Vec<String> {
    vec![
        r#"<<call send_email {"to":["a@b.c"],"subject":"s","body":"b"}>>"#.into(),
        r#"before <<call ping {}>> middle <<call send_email {"to":[],"subject":"a >> b","body":"}}\"{"}>> after"#.into(),
        r#"<<call send_email {"to":["é"],"subject":"😀 é \\ \"","body":"x","prio":"high"}>>"#.into(),
        "a << b, c <<< d <<cal <<call".into(),
        "<<call nope {}>>".into(),
        r#"<<call send_email {"to":["a"],"subject":"s"}>>"#.into(),
        r#"<<call send_email {"to":["a"],"subject":"s","body":"b"}>"#.into(),
        r#"<<call ping {}>><<call ping {}>>"#.into(),
    ]
}

type Outcome = Result<Vec<ToolCall>, DecodeError>;

fn run(tools: &[ToolDef], chunks: &[&str]) -> Outcome {
    let mut decoder = StreamDecoder::new(tools);
    let mut calls = Vec::new();
    for chunk in chunks {
        calls.extend(decoder.push(chunk)?);
    }
    calls.extend(decoder.finish()?);
    Ok(calls)
}

fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect()
}

#[test]
fn every_two_chunk_split_equals_the_batch_result() {
    let tools = tools();
    for text in corpus() {
        let batch = decode_calls(&text, &tools);
        for at in boundaries(&text) {
            let (a, b) = text.split_at(at);
            assert_eq!(run(&tools, &[a, b]), batch, "{text:?} split at {at}");
        }
    }
}

#[test]
fn one_char_per_chunk_equals_the_batch_result() {
    let tools = tools();
    for text in corpus() {
        let pieces: Vec<String> = text.chars().map(String::from).collect();
        let refs: Vec<&str> = pieces.iter().map(String::as_str).collect();
        assert_eq!(run(&tools, &refs), decode_calls(&text, &tools), "{text:?}");
    }
}

#[test]
fn a_cut_inside_a_call_is_an_error_at_finish() {
    let tools = tools();
    let text = r#"ok <<call send_email {"to":["a"],"subject":"s","body":"b"}>>"#;
    let start = text.find("<<call ").unwrap();
    let end = text.len();
    for at in boundaries(text) {
        if at < start + "<<call ".len() || at >= end {
            continue; // cut before the opener is complete, or after the call closed
        }
        let got = decode_calls(&text[..at], &tools);
        assert_eq!(
            got.map_err(|e| e.code()),
            Err("invalid_arguments"),
            "cut at {at}"
        );
    }
}

/// Deterministic partition from arbitrary cut points.
fn partition<'a>(text: &'a str, cuts: &[usize]) -> Vec<&'a str> {
    let marks = boundaries(text);
    let mut points: Vec<usize> = cuts.iter().map(|c| marks[c % marks.len()]).collect();
    points.sort_unstable();
    points.dedup();
    let mut out = Vec::new();
    let mut prev = 0;
    for p in points {
        out.push(&text[prev..p]);
        prev = p;
    }
    out.push(&text[prev..]);
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1500))]

    #[test]
    fn random_partitions_equal_the_batch_result(
        idx in 0usize..8,
        cuts in proptest::collection::vec(0usize..512, 0..12),
    ) {
        let tools = tools();
        let corpus = corpus();
        let text = &corpus[idx % corpus.len()];
        let parts = partition(text, &cuts);
        prop_assert_eq!(parts.concat(), text.clone());
        prop_assert_eq!(run(&tools, &parts), decode_calls(text, &tools));
    }

    #[test]
    fn arbitrary_text_never_panics_and_streaming_matches_batch(
        text in "(<<call |[ -~]|é|😀|\\n){0,60}",
        cuts in proptest::collection::vec(0usize..512, 0..6),
    ) {
        let tools = tools();
        let parts = partition(&text, &cuts);
        prop_assert_eq!(run(&tools, &parts), decode_calls(&text, &tools));
    }

    #[test]
    fn mutated_valid_calls_never_yield_a_call_with_an_error(
        idx in 0usize..8,
        pos in 0usize..512,
        junk in prop_oneof![Just('}'), Just('"'), Just('{'), Just('>'), Just(','), Just('x')],
    ) {
        // Whatever the mutation, the result is Ok(calls that validate) or Err; never both.
        let tools = tools();
        let corpus = corpus();
        let base = &corpus[idx % corpus.len()];
        let marks = boundaries(base);
        let at = marks[pos % marks.len()];
        let mut mutated = base.clone();
        mutated.insert(at, junk);
        let mut decoder = StreamDecoder::new(&tools);
        let mut emitted = 0usize;
        let mut failed = false;
        if let Ok(calls) = decoder.push(&mutated) { emitted += calls.len(); } else { failed = true; }
        if !failed && decoder.finish().is_err() { failed = true; }
        if failed {
            // Poisoned: nothing more may come out.
            prop_assert!(decoder.push(base).is_err());
            prop_assert!(decoder.finish().is_err());
        } else {
            prop_assert_eq!(emitted, decode_calls(&mutated, &tools).unwrap().len());
        }
    }
}
