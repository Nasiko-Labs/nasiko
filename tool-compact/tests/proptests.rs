use nasiko_tool_compact::{
    StreamDecoder, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools, render_calls,
};
use proptest::prelude::*;
use serde_json::json;

proptest! {
    #[test]
    fn prop_roundtrip_calls(name in "[a-zA-Z_][a-zA-Z0-9_]{1,15}", city in "[a-zA-Z ]{1,20}") {
        let tools = vec![ToolDef::new(
            &name,
            Some("Tool description".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            })),
        )];

        let original_call = vec![ToolCall::new(&name, json!({ "city": city }))];
        let rendered = render_calls(&original_call);
        let decoded = decode_calls(&rendered, &tools).unwrap();
        prop_assert_eq!(&decoded, &original_call);
    }

    #[test]
    fn prop_roundtrip_schemas(
        name in "[a-zA-Z_][a-zA-Z0-9_]{1,15}",
        min in 1i64..50,
        max in 51i64..100
    ) {
        let tools = vec![ToolDef::new(
            &name,
            Some("Calculate and search".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "count": { "type": "integer", "minimum": min, "maximum": max }
                },
                "required": ["count"]
            })),
        )];

        let compact = encode_tools(&tools).unwrap();
        let decoded = decode_tools(&compact).unwrap();
        prop_assert_eq!(decoded.len(), 1);
        prop_assert_eq!(&decoded[0].name, &name);
        let params = decoded[0].parameters.as_ref().unwrap();
        prop_assert_eq!(&params["properties"]["count"]["type"], "integer");
        prop_assert_eq!(&params["properties"]["count"]["minimum"], &json!(min));
        prop_assert_eq!(&params["properties"]["count"]["maximum"], &json!(max));
    }

    #[test]
    fn prop_random_stream_chunking(
        chunk_lens in proptest::collection::vec(1usize..10, 1..20),
        val in -1000i64..1000i64
    ) {
        let tools = vec![ToolDef::new(
            "calc",
            Some("Calculate something".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "num": { "type": "integer" }
                },
                "required": ["num"]
            })),
        )];

        let full_text = format!("Prefix text before call <<call calc {{\"num\": {val}}}>> suffix text after.");
        let mut decoder = StreamDecoder::new(tools);

        let mut offset = 0;
        for len in chunk_lens {
            if offset >= full_text.len() {
                break;
            }
            let end = (offset + len).min(full_text.len());
            let chunk = &full_text[offset..end];
            offset = end;
            let _ = decoder.push(chunk);
        }
        if offset < full_text.len() {
            let _ = decoder.push(&full_text[offset..]);
        }

        let calls = decoder.finish().unwrap();
        prop_assert_eq!(calls.len(), 1);
        prop_assert_eq!(&calls[0].name, "calc");
        prop_assert_eq!(&calls[0].arguments, &json!({ "num": val }));
    }

    #[test]
    fn prop_invalid_args_never_yield_call(invalid_num in "[a-zA-Z]{1,10}") {
        let tools = vec![ToolDef::new(
            "calc",
            Some("Calculate".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "num": { "type": "integer" }
                },
                "required": ["num"]
            })),
        )];

        let text = format!("<<call calc {{\"num\": \"{invalid_num}\"}}>>");
        let result = decode_calls(&text, &tools);
        prop_assert!(result.is_err());
    }
}
