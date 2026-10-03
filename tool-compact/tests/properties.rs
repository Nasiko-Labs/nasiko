use proptest::prelude::*;
use nasiko_tool_compact::{decode_response, encode_tools, Limits, StreamDecoder, ToolDef};
use serde_json::json;

proptest! {
    #[test]
    fn parser_never_panics_for_arbitrary_bytes(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let tools = vec![
            ToolDef::new(
                "fuzz_tool",
                None,
                json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "string" }
                    }
                })
            )
        ];
        let mut decoder = StreamDecoder::new(&tools, Limits::default()).unwrap();
        let _ = decoder.push(&data);
        let _ = decoder.finish();
    }

    #[test]
    fn repeated_encoding_produces_identical_text(s in "[a-zA-Z0-9_]{1,20}") {
        let tools = vec![
            ToolDef::new(
                s.clone(),
                Some("A deterministic description".to_string()),
                json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "count": { "type": "integer" }
                    },
                    "required": ["name"]
                })
            )
        ];
        let enc1 = encode_tools(&tools).unwrap();
        let enc2 = encode_tools(&tools).unwrap();
        assert_eq!(enc1.text, enc2.text);
    }

    #[test]
    fn chunk_partitions_match_batch(chunk_size in 1usize..32) {
        let tools = vec![
            ToolDef::new(
                "test_func",
                None,
                json!({
                    "type": "object",
                    "properties": { "msg": { "type": "string" } },
                    "required": ["msg"]
                })
            )
        ];
        let sample = "Answer: <<call test_func {\"msg\":\"hello streaming world\"}>> Done.";
        let batch = decode_response(sample, &tools).unwrap();

        let mut streaming = StreamDecoder::new(&tools, Limits::default()).unwrap();
        for chunk in sample.as_bytes().chunks(chunk_size) {
            streaming.push(chunk).unwrap();
        }
        let stream_res = streaming.finish().unwrap();
        assert_eq!(batch, stream_res);
    }
}

#[test]
fn adding_unsupported_keyword_never_silently_produces_compact_mode() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "prop": { "type": "string" }
        }
    });

    let tools = vec![ToolDef::new("valid_tool", None, schema.clone())];
    assert!(encode_tools(&tools).is_ok());

    // Inject an unsupported keyword:
    schema.as_object_mut().unwrap().insert("const".to_string(), json!("illegal"));
    let unsupported_tools = vec![ToolDef::new("invalid_tool", None, schema)];
    let err = encode_tools(&unsupported_tools).unwrap_err();
    assert!(err.should_bypass());
}
