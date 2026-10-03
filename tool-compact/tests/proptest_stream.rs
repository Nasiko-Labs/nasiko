//! Property-based tests for the streaming decoder.
//!
//! Verifies that splitting valid model output at arbitrary positions
//! produces the same decoded calls as the unsplit baseline.

use nasiko_tool_compact::{StreamDecoder, ToolDef, encode_tools, decode_calls};
use nasiko_tool_compact::types::FunctionDef;
use proptest::prelude::*;
use serde_json::json;

fn search_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "search".into(),
            description: Some("Search the web.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            })),
        },
        extra: serde_json::Map::new(),
    }
}

fn translate_tool() -> ToolDef {
    ToolDef {
        kind: "function".into(),
        function: FunctionDef {
            name: "translate".into(),
            description: Some("Translate text.".into()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "lang": { "type": "string", "enum": ["en", "fr", "de"] }
                },
                "required": ["text", "lang"]
            })),
        },
        extra: serde_json::Map::new(),
    }
}

/// Split a string at the given byte positions.
fn split_at_positions(s: &str, positions: &[usize]) -> Vec<String> {
    let mut sorted: Vec<usize> = positions
        .iter()
        .copied()
        .filter(|&p| p > 0 && p < s.len())
        .collect();
    sorted.sort_unstable();
    sorted.dedup();

    let mut chunks = Vec::new();
    let mut start = 0;
    for &pos in &sorted {
        // Only split at valid char boundaries.
        if s.is_char_boundary(pos) && pos > start {
            chunks.push(s[start..pos].to_string());
            start = pos;
        }
    }
    chunks.push(s[start..].to_string());
    chunks
}

proptest! {
    #[test]
    fn random_split_single_call(
        split_points in prop::collection::vec(0..100usize, 1..15)
    ) {
        let tools = vec![search_tool()];
        let compact = encode_tools(&tools).unwrap();
        let full_text = r#"Here is the answer. <<call search {"query":"proptest"}>>"#;

        // Baseline: unsplit decode.
        let baseline = decode_calls(full_text, &tools).unwrap();

        // Split at random positions and feed to StreamDecoder.
        let chunks = split_at_positions(full_text, &split_points);
        let mut decoder = StreamDecoder::new(&tools, &compact.schemas);
        for chunk in &chunks {
            decoder.push(chunk);
        }
        let streamed = decoder.finish().unwrap();

        // Same calls.
        prop_assert_eq!(baseline.len(), streamed.len());
        for (b, s) in baseline.iter().zip(streamed.iter()) {
            prop_assert_eq!(&b.function.name, &s.function.name);
            prop_assert_eq!(&b.function.arguments, &s.function.arguments);
        }
    }

    #[test]
    fn random_split_multiple_calls(
        split_points in prop::collection::vec(0..200usize, 1..25)
    ) {
        let tools = vec![search_tool(), translate_tool()];
        let compact = encode_tools(&tools).unwrap();
        let full_text = r#"<<call search {"query":"hello"}>> And also <<call translate {"text":"hi","lang":"fr"}>>"#;

        let baseline = decode_calls(full_text, &tools).unwrap();

        let chunks = split_at_positions(full_text, &split_points);
        let mut decoder = StreamDecoder::new(&tools, &compact.schemas);
        for chunk in &chunks {
            decoder.push(chunk);
        }
        let streamed = decoder.finish().unwrap();

        prop_assert_eq!(baseline.len(), streamed.len());
        for (b, s) in baseline.iter().zip(streamed.iter()) {
            prop_assert_eq!(&b.function.name, &s.function.name);
            prop_assert_eq!(&b.function.arguments, &s.function.arguments);
        }
    }

    #[test]
    fn random_split_no_calls(
        split_points in prop::collection::vec(0..50usize, 1..10)
    ) {
        let tools = vec![search_tool()];
        let compact = encode_tools(&tools).unwrap();
        let full_text = "Just regular text without any tool calls at all.";

        let chunks = split_at_positions(full_text, &split_points);
        let mut decoder = StreamDecoder::new(&tools, &compact.schemas);
        for chunk in &chunks {
            decoder.push(chunk);
        }
        let streamed = decoder.finish().unwrap();
        prop_assert!(streamed.is_empty());
    }
}
