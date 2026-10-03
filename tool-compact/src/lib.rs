//! # nasiko-tool-compact
//!
//! Lossless compact tool schemas with a fail-closed decoder and streaming parser for LLM tool calling.
//!
//! ## Grammar (EBNF)
//!
//! ```ebnf
//! Call           ::= "<<" "call" WS ToolName WS JsonObject WS? ">>"
//! ToolName       ::= [a-zA-Z0-9_-]+
//! JsonObject     ::= "{" JsonMembers? "}"
//! JsonMembers    ::= JsonMember ("," JsonMember)*
//! JsonMember     ::= String WS? ":" WS? JsonValue
//! JsonValue      ::= String | Number | JsonObject | JsonArray | "true" | "false" | "null"
//! JsonArray      ::= "[" (JsonValue ("," JsonValue)*)? "]"
//! WS             ::= [ \t\r\n]+
//! String         ::= '"' ([^"\\\x00-\x1f] | Escape)* '"'
//! Escape         ::= '\' (["\\/bfnrt] | "u" [0-9a-fA-F]{4})
//! ```
//!
//! The scanner is fully JSON-aware: after `<<call NAME `, it tracks string escape sequences
//! and brace depth `{ ... }`. Any `>>` or `<<` inside JSON string literals does NOT close
//! or open a call. Assistant text preceding, between, or following calls is preserved.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::string_slice
    )
)]

pub mod calls;
pub mod decode_schema;
pub mod encode;
pub(crate) mod slice;
pub mod stream;
pub mod types;

pub use calls::{
    decode_calls, render_calls, scan_call_spans, scan_raw_calls, validate_tool_arguments,
};
pub use decode_schema::{decode_tool_signature, decode_tools};
pub use encode::{
    encode_tools, encode_tools_with_variant, extract_first_sentence, render_tool_signature,
    should_keep_param_desc,
};
pub use stream::{StreamDecoder, StreamEvent};
pub use types::{CompactError, CompactTools, InstructionVariant, ToolCall, ToolDef};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef::new(
                "get_weather",
                Some(
                    "Get the current weather for a city. Returns temperature and conditions."
                        .to_string(),
                ),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string", "description": "City name" },
                        "units": { "type": "string", "enum": ["metric", "imperial"], "default": "metric", "description": "Temperature units to return" }
                    },
                    "required": ["city"]
                })),
            ),
            ToolDef::new(
                "search_files",
                Some("Search codebase files for regex pattern.".to_string()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "max_results": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                    },
                    "required": ["pattern"]
                })),
            ),
        ]
    }

    #[test]
    fn test_encode_and_decode_roundtrip() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.text.contains("get_weather(city:str, units?:metric|imperial=\"metric\" /* Temperature units to return */) - Get the current weather for a city."));
        assert!(compact.text.contains("search_files(pattern:str, max_results?:int(1..100)=10) - Search codebase files for regex pattern."));

        let decoded = decode_tools(&compact).unwrap();
        assert_eq!(decoded.len(), tools.len());
        assert_eq!(decoded[0].name, tools[0].name);
        assert_eq!(decoded[1].name, tools[1].name);

        let p0 = decoded[0].parameters.as_ref().unwrap();
        assert_eq!(p0["properties"]["city"]["type"], "string");
        assert_eq!(p0["required"], json!(["city"]));
        assert_eq!(
            p0["properties"]["units"]["enum"],
            json!(["metric", "imperial"])
        );
    }

    #[test]
    fn test_decode_calls_single_and_multiple() {
        let tools = sample_tools();
        let text = "I will check the weather for London:\n<<call get_weather {\"city\": \"London\", \"units\": \"metric\"}>>\nAnd also Paris:\n<<call get_weather {\"city\": \"Paris\"}>>\nDone!";
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(
            calls[0].arguments,
            json!({"city": "London", "units": "metric"})
        );
        assert_eq!(calls[1].name, "get_weather");
        assert_eq!(calls[1].arguments, json!({"city": "Paris"}));
    }

    #[test]
    fn test_json_with_brackets_and_escapes_inside_string() {
        let tools = vec![ToolDef::new(
            "echo",
            Some("Echo text".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            })),
        )];

        let text = r#"Result: <<call echo {"text": "hello >> << world \"quoted\" and {nested braces}"}>> Thank you"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "echo");
        assert_eq!(
            calls[0].arguments["text"],
            "hello >> << world \"quoted\" and {nested braces}"
        );
    }

    #[test]
    fn test_unknown_tool_fails_closed() {
        let tools = sample_tools();
        let text = "<<call unknown_tool {\"foo\": \"bar\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::UnknownTool(name) if name == "unknown_tool"));
    }

    #[test]
    fn test_invalid_arguments_type_fails_closed() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"city\": 12345}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn test_missing_required_property_fails_closed() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"units\": \"metric\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(
            matches!(err, CompactError::InvalidArguments { tool, reason } if tool == "get_weather" && reason.contains("city"))
        );
    }

    #[test]
    fn test_enum_violation_fails_closed() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"city\": \"Tokyo\", \"units\": \"kelvin\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn test_plain_answer_no_calls() {
        let tools = sample_tools();
        let text = "The weather today in Berlin is 18°C and sunny.";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_stream_decoder_across_random_chunkings() {
        let tools = sample_tools();
        let stream_text = "Checking now...\n<<call get_weather {\"city\": \"Berlin\", \"units\": \"metric\"}>>\nAll set.";

        // Test every single character split boundary
        for split_point in 1..stream_text.len() {
            let mut decoder = StreamDecoder::new(tools.clone());
            let (part1, part2) = stream_text.split_at(split_point);
            let _ = decoder.push(part1);
            let _ = decoder.push(part2);
            let calls = decoder.finish().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, "get_weather");
            assert_eq!(
                calls[0].arguments,
                json!({"city": "Berlin", "units": "metric"})
            );
        }
    }

    #[test]
    fn test_stream_decoder_truncated_call_fails() {
        let tools = sample_tools();
        let mut decoder = StreamDecoder::new(tools);
        let _ = decoder.push("Starting <<call get_weather {\"city\": \"Ber");
        let err = decoder.finish().unwrap_err();
        assert!(matches!(err, CompactError::InvalidArguments { .. }));
    }

    #[test]
    fn test_unsupported_schema_rejected() {
        let tools = vec![ToolDef::new(
            "bad_tool",
            Some("Tool with $ref".to_string()),
            Some(json!({
                "type": "object",
                "properties": {
                    "item": { "$ref": "#/definitions/Item" }
                }
            })),
        )];

        let err = encode_tools(&tools).unwrap_err();
        assert!(
            matches!(err, CompactError::Unsupported { tool, feature } if tool == "bad_tool" && feature == "$ref")
        );
    }
}
