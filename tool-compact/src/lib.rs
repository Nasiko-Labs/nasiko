//! # Nasiko Compact Tool Schemas & Streaming Protocol
//!
//! Pure library crate providing:
//! - Deterministic Compact DSL schema encoding (`encode_tools`)
//! - Deterministic state machine tool call decoding (`decode_calls`)
//! - Chunk-agnostic streaming parser (`StreamDecoder`)
//! - Fail-closed schema & type validation

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod stream;
pub mod types;
pub mod validator;

pub use decoder::{decode_calls, decode_output};
pub use encoder::{encode_single_tool, encode_tools, format_type_schema, parse_json_schema};
pub use error::CompactToolError;
pub use stream::StreamDecoder;
pub use types::{
    CompactTools, DecodeOutput, DecodedToolCall, PropertySchema, StreamEvent, ToolCall, ToolDef,
    ToolDefinition, TypeSchema,
};
pub use validator::validate_tool_call;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition::new(
                "get_weather",
                Some("Fetch current weather conditions".into()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name" },
                        "unit": { "type": "string", "enum": ["celsius", "fahrenheit"], "description": "Temperature unit" }
                    },
                    "required": ["location"],
                    "additionalProperties": false
                })),
            ),
            ToolDefinition::new(
                "execute_command",
                Some("Execute a shell command".into()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "cmd": { "type": "string" },
                        "timeout_sec": { "type": "integer", "default": 30 }
                    },
                    "required": ["cmd"]
                })),
            ),
            ToolDefinition::new(
                "tag_issue",
                Some("Apply labels to an issue".into()),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "issue_id": { "type": "integer" },
                        "tags": { "type": "array", "items": { "type": "string" } },
                        "metadata": {
                            "type": "object",
                            "properties": {
                                "priority": { "type": "string", "enum": ["low", "high"] }
                            },
                            "required": ["priority"]
                        }
                    },
                    "required": ["issue_id", "tags"]
                })),
            ),
        ]
    }

    #[test]
    fn test_encode_tools_primitives_and_enums() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).expect("should encode successfully");
        let prompt = compact.as_str();

        assert!(prompt.contains("tool get_weather("));
        assert!(prompt.contains("location: string [City name]"));
        assert!(prompt.contains("unit?: \"celsius\" | \"fahrenheit\" [Temperature unit]"));
        assert!(prompt.contains("tool execute_command("));
        assert!(prompt.contains("cmd: string"));
        assert!(prompt.contains("timeout_sec?: integer = 30"));
        assert!(prompt.contains("tags: string[]"));
        assert!(prompt.contains("metadata?: { priority: \"low\" | \"high\" }"));
        assert!(prompt.contains("<<call tool_name {\"arg\": \"value\"}>>"));
    }

    #[test]
    fn test_encode_tools_deterministic() {
        let tools = sample_tools();
        let res1 = encode_tools(&tools).unwrap();
        let res2 = encode_tools(&tools).unwrap();
        assert_eq!(res1, res2);
    }

    #[test]
    fn test_unsupported_schema_rejected() {
        let unsupported_cases = vec![
            (
                "oneOf",
                json!({ "type": "object", "oneOf": [{ "type": "string" }] }),
            ),
            (
                "anyOf",
                json!({ "type": "object", "anyOf": [{ "type": "string" }] }),
            ),
            (
                "allOf",
                json!({ "type": "object", "allOf": [{ "type": "string" }] }),
            ),
            ("$ref", json!({ "$ref": "#/definitions/Foo" })),
            ("$defs", json!({ "$defs": { "Foo": { "type": "string" } } })),
            (
                "patternProperties",
                json!({ "type": "object", "patternProperties": { "^s_": { "type": "string" } } }),
            ),
            (
                "prefixItems",
                json!({ "type": "array", "prefixItems": [{ "type": "string" }] }),
            ),
            ("if", json!({ "if": { "type": "string" } })),
            (
                "schema_additional_props",
                json!({ "type": "object", "additionalProperties": { "type": "string" } }),
            ),
        ];

        for (name, schema) in unsupported_cases {
            let tool = ToolDefinition::new("bad_tool", None, Some(schema));
            let err = encode_tools(&[tool]).unwrap_err();
            assert!(
                matches!(err, CompactToolError::UnsupportedSchemaFeature { .. }),
                "Case '{}' should fail with UnsupportedSchemaFeature, got: {:?}",
                name,
                err
            );
        }
    }

    #[test]
    fn test_decode_single_call() {
        let tools = sample_tools();
        let text = "Checking weather: <<call get_weather {\"location\":\"London\",\"unit\":\"celsius\"}>> done.";
        let out = decode_output(text, &tools).expect("should decode successfully");

        assert_eq!(out.text, "Checking weather:  done.");
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].name, "get_weather");
        let args: serde_json::Value = serde_json::from_str(&out.tool_calls[0].arguments).unwrap();
        assert_eq!(args["location"], "London");
        assert_eq!(args["unit"], "celsius");
    }

    #[test]
    fn test_decode_multiple_calls() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"location\":\"Tokyo\"}>>\n<<call execute_command {\"cmd\":\"ls -la\"}>>";
        let calls = decode_calls(text, &tools).expect("should decode multiple calls");

        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[1].name, "execute_command");
    }

    #[test]
    fn test_decode_no_calls() {
        let tools = sample_tools();
        let text = "Just plain text without any tool calls.";
        let out = decode_output(text, &tools).unwrap();
        assert_eq!(out.text, text);
        assert!(out.tool_calls.is_empty());
    }

    #[test]
    fn test_decode_false_start_markers() {
        let tools = sample_tools();
        let text = "In C++, 1 << 2 is 4, and 8 >> 1 is 4. Also <<cal and <<callbad {}>>.";
        let out = decode_output(text, &tools).unwrap();
        assert!(out.tool_calls.is_empty());
        assert!(out.text.contains("1 << 2"));
    }

    #[test]
    fn test_decode_literal_angle_brackets_inside_json_string() {
        let tools = sample_tools();
        let text = "<<call execute_command {\"cmd\":\"echo 'hello' >> /tmp/out.log && cat >> file.txt\"}>>";
        let calls = decode_calls(text, &tools).expect("should handle >> inside string");

        assert_eq!(calls.len(), 1);
        let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(
            args["cmd"],
            "echo 'hello' >> /tmp/out.log && cat >> file.txt"
        );
    }

    #[test]
    fn test_decode_escaped_quotes_and_backslashes() {
        let tools = sample_tools();
        let text = "<<call execute_command {\"cmd\":\"echo \\\"Hello \\\\\\\"World\\\\\\\"\\\" >> /dev/null\"}>>";
        let calls = decode_calls(text, &tools).expect("should handle escaped quotes");
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn test_fail_closed_unknown_tool() {
        let tools = sample_tools();
        let text = "<<call non_existent_tool {\"a\":1}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(
            err,
            CompactToolError::UnknownTool {
                tool_name: "non_existent_tool".into()
            }
        );
    }

    #[test]
    fn test_fail_closed_missing_required() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"unit\":\"celsius\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(
            err,
            CompactToolError::MissingRequiredField {
                tool_name: "get_weather".into(),
                field: "location".into()
            }
        );
    }

    #[test]
    fn test_fail_closed_invalid_type() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"location\": 12345}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactToolError::InvalidFieldType { .. }));
    }

    #[test]
    fn test_fail_closed_invalid_enum() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"location\":\"Paris\",\"unit\":\"kelvin\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(
            err,
            CompactToolError::InvalidEnumValue {
                tool_name: "get_weather".into(),
                field: "unit".into(),
                expected: vec!["celsius".into(), "fahrenheit".into()],
                found: "kelvin".into(),
            }
        );
    }

    #[test]
    fn test_fail_closed_unexpected_field_on_strict_tool() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"location\":\"Paris\",\"extra_foo\":\"bar\"}>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(
            err,
            CompactToolError::UnexpectedField {
                tool_name: "get_weather".into(),
                field: "extra_foo".into()
            }
        );
    }

    #[test]
    fn test_fail_closed_malformed_json() {
        let tools = sample_tools();
        let text = "<<call get_weather {\"location\": \"Paris\", }>>";
        let err = decode_calls(text, &tools).unwrap_err();
        assert!(matches!(err, CompactToolError::MalformedJson { .. }));
    }

    #[test]
    fn test_fail_closed_unclosed_stream() {
        let tools = sample_tools();
        let mut stream = StreamDecoder::new(tools);
        stream
            .feed("Starting: <<call get_weather {\"location\":\"Paris\"")
            .unwrap();
        let err = stream.finish().unwrap_err();
        assert_eq!(err, CompactToolError::UnclosedCallMarker);
    }

    #[test]
    fn test_streaming_chunk_boundary_permutations() {
        let tools = sample_tools();
        let full_text = "Before text. <<call get_weather {\"location\":\"Berlin\",\"unit\":\"celsius\"}>> Middle. <<call execute_command {\"cmd\":\"echo >> test\"}>> After text.";

        // Validate full text decode baseline
        let baseline_calls = decode_calls(full_text, &tools).unwrap();
        let baseline_output = decode_output(full_text, &tools).unwrap();

        // Test chunk sizes from 1 byte to full_text.len()
        for chunk_size in 1..=20 {
            let mut stream = StreamDecoder::new(tools.clone());
            let mut collected_events = Vec::new();

            for chunk in full_text.as_bytes().chunks(chunk_size) {
                let chunk_str = std::str::from_utf8(chunk).unwrap();
                let events = stream.feed(chunk_str).unwrap();
                collected_events.extend(events);
            }

            let final_events = stream.finish().unwrap();
            collected_events.extend(final_events);

            assert_eq!(
                stream.completed_calls().len(),
                baseline_calls.len(),
                "Mismatch at chunk_size {}",
                chunk_size
            );

            for (i, call) in stream.completed_calls().iter().enumerate() {
                assert_eq!(call.name, baseline_calls[i].name);
                assert_eq!(call.arguments, baseline_calls[i].arguments);
            }

            assert_eq!(
                stream.current_text(),
                baseline_output.text,
                "Text mismatch at chunk_size {}",
                chunk_size
            );
        }
    }
}
