//! Compact tool definition format and decoder for LLM tool calling token reduction.
//!
//! # Grammar (EBNF)
//! ```ebnf
//! compact_block  = header LF {tool_line LF} LF instructions LF example
//! header         = "# Tools"
//! tool_line      = name "(" params ")" [" - " description]
//! params         = param {", " param}
//! param          = field_name ["?"] ":" type
//! type           = "str" | "int" | "num" | "bool" | "datetime" | "date"
//!                | "[" type "]"                    (* array *)
//!                | enum_list                       (* string enum *)
//!                | "{" params "}"                  (* nested object *)
//! enum_list      = value {"|" value}
//! instructions   = 'To call a tool, emit: <<call name {"arg":"val"}>>'
//! example        = "Example: <<call " name " " json_args ">>"
//! ```
//!
//! # Supported Schema Subset
//! - Primitive types: `string`, `integer`, `number`, `boolean`
//! - Formats: `date-time`, `date`
//! - String enums (alphanumeric string values without whitespace, quotes or `|`)
//! - Arrays with typed elements
//! - Nested objects with required/optional fields
//!
//! # Unsupported Feature Bypass List
//! Tools using any of the following schema keywords or patterns bypass compaction:
//! - `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, `if`, `then`, `else`
//! - `patternProperties`, non-boolean `additionalProperties`
//! - Format-less constraints: `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`,
//!   `minLength`, `maxLength`, `pattern`, `default`, `const`, `multipleOf`, `uniqueItems`,
//!   `minItems`, `maxItems`, `minProperties`, `maxProperties`
//! - Non-string enums or string enums containing whitespace, quotes, or `|`
//!
//! # Failure Policy
//! - Fail closed: unknown tool or invalid argument produces a typed error (`UnknownTool` or `InvalidArguments`).
//! - No silent guessing or mutation of calls.
//! - Invalid or truncated call markers return a `MalformedOutput` error.

#![forbid(unsafe_code)]
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod decode;
pub mod encode;
pub mod error;
pub mod render;
pub mod schema;
pub mod stream;
pub mod types;
pub mod validate;

pub use decode::decode_calls;
pub use encode::{decode_tools, encode_tools};
pub use error::CompactError;
pub use render::{render_call, render_calls};
pub use stream::StreamDecoder;
pub use types::{CompactTools, FunctionCall, FunctionDef, ToolCall, ToolDef};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_tools() -> Vec<ToolDef> {
        vec![
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "create_calendar_event".into(),
                    description: Some("Create an event in the user's calendar.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "start": { "type": "string", "format": "date-time" },
                            "duration_min": { "type": "integer" },
                            "attendees": { "type": "array", "items": { "type": "string" } },
                            "visibility": { "type": "string", "enum": ["public", "private"] }
                        },
                        "required": ["title", "start"]
                    })),
                },
                extra: Default::default(),
            },
            ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "send_email".into(),
                    description: Some("Send an email.".into()),
                    parameters: Some(json!({
                        "type": "object",
                        "properties": {
                            "to": { "type": "string" },
                            "subject": { "type": "string" },
                            "body": { "type": "string" }
                        },
                        "required": ["to", "subject", "body"]
                    })),
                },
                extra: Default::default(),
            },
        ]
    }

    #[test]
    fn test_encode_and_decode_roundtrip() {
        let tools = sample_tools();
        let compact = encode_tools(&tools).unwrap();
        assert!(compact.compacted);
        assert!(compact.text.contains("create_calendar_event"));

        let reconstructed = decode_tools(&compact).unwrap();
        assert_eq!(reconstructed.len(), 2);
    }

    #[test]
    fn test_valid_decoding() {
        let tools = sample_tools();
        let text = r#"Here is the call: <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(calls[0].id, "call_1");
    }

    #[test]
    fn test_edge_case_unknown_tool() {
        let tools = sample_tools();
        let text = r#"<<call unknown_tool {}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_tag(), "unknown_tool");
    }

    #[test]
    fn test_edge_case_missing_required_field() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Missing start"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_tag(), "invalid_arguments");
    }

    #[test]
    fn test_edge_case_enum_violation() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Test","start":"2026-10-05T15:00:00Z","visibility":"secret"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_tag(), "invalid_arguments");
    }

    #[test]
    fn test_edge_case_wrong_type() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":123,"start":"2026-10-05T15:00:00Z"}>>"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_tag(), "invalid_arguments");
    }

    #[test]
    fn test_edge_case_gt_inside_string() {
        let tools = sample_tools();
        let text =
            r#"<<call create_calendar_event {"title":"A >> B","start":"2026-10-05T15:00:00Z"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], "A >> B");
    }

    #[test]
    fn test_edge_case_escaped_quotes_and_backslashes() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Title with \"quotes\" and \\backslash","start":"2026-10-05T15:00:00Z"}>>"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        let args: serde_json::Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["title"], r#"Title with "quotes" and \backslash"#);
    }

    #[test]
    fn test_edge_case_multiple_calls_and_surrounding_text() {
        let tools = sample_tools();
        let text = r#"I will schedule two events. First:
<<call create_calendar_event {"title":"Meeting 1","start":"2026-10-05T15:00:00Z"}>>
And second:
<<call send_email {"to":"alice@example.com","subject":"Hi","body":"Hello"}>>
Done!"#;
        let calls = decode_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "create_calendar_event");
        assert_eq!(calls[1].function.name, "send_email");
    }

    #[test]
    fn test_edge_case_no_call() {
        let tools = sample_tools();
        let text = "Just a plain conversational answer with no calls.";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }

    #[test]
    fn test_edge_case_truncated_call() {
        let tools = sample_tools();
        let text = r#"<<call create_calendar_event {"title":"Meeting"#;
        let err = decode_calls(text, &tools).unwrap_err();
        assert_eq!(err.eval_tag(), "invalid_arguments");
    }

    #[test]
    fn test_edge_case_unsupported_feature_bypass() {
        let mut tools = sample_tools();
        tools[0].function.parameters = Some(json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "minimum": 1 }
            }
        }));
        let compact = encode_tools(&tools).unwrap();
        assert!(!compact.tool_status[0].compacted);
    }

    #[test]
    fn test_lone_angle_bracket_is_plain_text() {
        let tools = sample_tools();
        let text = "5 < 10 and 10 > 5 and << incomplete text";
        let calls = decode_calls(text, &tools).unwrap();
        assert!(calls.is_empty());
    }
}
