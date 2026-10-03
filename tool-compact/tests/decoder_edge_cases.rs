//! Comprehensive decoder and validation edge-case integration tests for nasiko-tool-compact.

use nasiko_tool_compact::{Error, FunctionDef, StreamDecoder, ToolDef, decode_calls, render_call};
use serde_json::{Value, json};

fn test_tool_suite() -> Vec<ToolDef> {
    vec![
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "search_docs".to_string(),
                description: Some("Search the documentation index.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query terms"},
                        "limit": {"type": "integer", "description": "Maximum results"},
                        "filter": {
                            "type": "object",
                            "properties": {
                                "category": {"type": "string", "enum": ["api", "guide", "faq"]},
                                "author": {"type": "string"}
                            },
                            "required": ["category"]
                        },
                        "tags": {
                            "type": "array",
                            "items": {"type": "string"}
                        }
                    },
                    "required": ["query"]
                })),
            },
        },
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "format_validator".to_string(),
                description: Some("Validates specific string formats.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "ts": {"type": "string", "format": "date-time"},
                        "dt": {"type": "string", "format": "date"},
                        "tm": {"type": "string", "format": "time"},
                        "email": {"type": "string", "format": "email"},
                        "link": {"type": "string", "format": "uri"},
                        "uuid": {"type": "string", "format": "uuid"},
                        "ratio": {"type": "number"},
                        "is_active": {"type": "boolean"}
                    }
                })),
            },
        },
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "no_params_tool".to_string(),
                description: Some("Tool with empty parameters schema.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {}
                })),
            },
        },
    ]
}

// =========================================================================
// 1. Chunk boundary and marker splitting tests
// =========================================================================

#[test]
fn test_single_byte_chunk_stream() {
    let tools = test_tool_suite();
    let payload = "<<call search_docs {\"query\": \"rust async\", \"limit\": 5}>>";

    let mut decoder = StreamDecoder::new(&tools).unwrap();
    for b in payload.bytes() {
        let b_arr = [b];
        let chunk = std::str::from_utf8(&b_arr).unwrap();
        decoder.push(chunk).unwrap();
    }
    let calls = decoder.finish().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search_docs");
    let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
    assert_eq!(args["query"], "rust async");
    assert_eq!(args["limit"], 5);
}

#[test]
fn test_marker_split_at_every_index() {
    let tools = test_tool_suite();
    let text = "<<call no_params_tool {}>>";

    // Split text at every single possible index [1..text.len()-1]
    for split_idx in 1..text.len() {
        let chunk1 = &text[..split_idx];
        let chunk2 = &text[split_idx..];

        let mut decoder = StreamDecoder::new(&tools).unwrap();
        decoder.push(chunk1).unwrap();
        decoder.push(chunk2).unwrap();
        let calls = decoder.finish().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "Failed when splitting at byte offset {split_idx}"
        );
        assert_eq!(calls[0].function.name, "no_params_tool");
    }
}

#[test]
fn test_false_marker_prefixes_in_text() {
    let tools = test_tool_suite();
    let stream_text =
        "< <c <<cal <<california <<calcium <<call search_docs {\"query\": \"test\"}>>";

    let calls = decode_calls(stream_text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search_docs");
}

#[test]
fn test_multiple_less_than_characters_before_call() {
    let tools = test_tool_suite();
    let text = "<<<<<call search_docs {\"query\": \"overflow\"}>>";
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "search_docs");
}

// =========================================================================
// 2. Unterminated call tests
// =========================================================================

#[test]
fn test_unterminated_calls() {
    let tools = test_tool_suite();

    let incomplete_cases = [
        "<<call",
        "<<call ",
        "<<call search_docs",
        "<<call search_docs ",
        "<<call search_docs {",
        "<<call search_docs {\"query\": \"incomplete\"",
        "<<call search_docs {\"query\": \"incomplete\"}",
        "<<call search_docs {\"query\": \"incomplete\"}>",
    ];

    for case in incomplete_cases {
        let mut decoder = StreamDecoder::new(&tools).unwrap();
        let push_res = decoder.push(case);
        if push_res.is_ok() {
            let finish_res = decoder.finish();
            assert!(
                matches!(finish_res, Err(Error::UnterminatedCall)),
                "Case '{case}' did not return UnterminatedCall, got: {:?}",
                finish_res
            );
        }
    }
}

// =========================================================================
// 3. Conversational text and multi-call tests
// =========================================================================

#[test]
fn test_conversational_text_wrapping_calls() {
    let tools = test_tool_suite();
    let text = r#"
Sure! Here is the plan.
First, I will run the search:
<<call search_docs {"query": "first step"}>>
Based on the results, we might search again:
<<call search_docs {"query": "second step", "limit": 10}>>
Done! Hope that helps.
"#;
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[1].id, "call_2");
    assert_eq!(calls[0].function.name, "search_docs");
    assert_eq!(calls[1].function.name, "search_docs");
}

#[test]
fn test_adjacent_calls_without_spacing() {
    let tools = test_tool_suite();
    let text = "<<call search_docs {\"query\": \"q1\"}>><<call search_docs {\"query\": \"q2\"}>>";
    let calls = decode_calls(text, &tools).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "search_docs");
    assert_eq!(calls[1].function.name, "search_docs");
}

// =========================================================================
// 4. Complex strings, quotes, braces, colons, arrows inside string values
// =========================================================================

#[test]
fn test_strings_with_special_tokens() {
    let tools = test_tool_suite();

    // Closing marker >> inside string
    let text1 = "<<call search_docs {\"query\": \"compare a >> b and c >> d\"}>>";
    let calls1 = decode_calls(text1, &tools).unwrap();
    let args1: Value = serde_json::from_str(&calls1[0].function.arguments).unwrap();
    assert_eq!(args1["query"], "compare a >> b and c >> d");

    // Braces and colons inside string
    let text2 = r#"<<call search_docs {"query": "{\"key\": \"val\", \"x\": [1,2]}"}>>"#;
    let calls2 = decode_calls(text2, &tools).unwrap();
    let args2: Value = serde_json::from_str(&calls2[0].function.arguments).unwrap();
    assert_eq!(args2["query"], "{\"key\": \"val\", \"x\": [1,2]}");

    // Escaped quotes inside string
    let text3 = "<<call search_docs {\"query\": \"She said \\\"Hello World!\\\"\"}>>";
    let calls3 = decode_calls(text3, &tools).unwrap();
    let args3: Value = serde_json::from_str(&calls3[0].function.arguments).unwrap();
    assert_eq!(args3["query"], "She said \"Hello World!\"");

    // Escaped backslashes
    let text4 = "<<call search_docs {\"query\": \"C:\\\\Program Files\\\\Nasiko\"}>>";
    let calls4 = decode_calls(text4, &tools).unwrap();
    let args4: Value = serde_json::from_str(&calls4[0].function.arguments).unwrap();
    assert_eq!(args4["query"], "C:\\Program Files\\Nasiko");
}

// =========================================================================
// 5. Strict duplicate key rejection
// =========================================================================

#[test]
fn test_duplicate_key_rejections() {
    let tools = test_tool_suite();

    // Root duplicate
    let bad_root = "<<call search_docs {\"query\": \"q1\", \"query\": \"q2\"}>>";
    let err = decode_calls(bad_root, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(err.to_string().contains("duplicate key 'query'"));

    // Nested duplicate
    let bad_nested = "<<call search_docs {\"query\": \"q\", \"filter\": {\"category\": \"api\", \"category\": \"guide\"}}>>";
    let err = decode_calls(bad_nested, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(err.to_string().contains("duplicate key 'category'"));

    // Array of objects with duplicate key
    let bad_array = "<<call search_docs {\"query\": \"q\", \"tags\": [\"t1\"], \"filter\": {\"category\": \"api\"}, \"extra\": [{\"k\": 1, \"k\": 2}]}>>";
    let err = decode_calls(bad_array, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");

    // Sibling objects having identical keys is VALID
    let ok_siblings =
        "<<call search_docs {\"query\": \"ok\", \"filter\": {\"category\": \"api\"}}>>";
    assert!(decode_calls(ok_siblings, &tools).is_ok());
}

// =========================================================================
// 6. Schema validation strictness & fail-closed behavior
// =========================================================================

#[test]
fn test_unknown_field_rejection_fail_closed() {
    let tools = test_tool_suite();
    let text = "<<call search_docs {\"query\": \"q\", \"non_existent_field\": 123}>>";
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(
        err.to_string()
            .contains("unknown field 'non_existent_field'")
    );
}

#[test]
fn test_nested_unknown_field_rejection() {
    let tools = test_tool_suite();
    let text = "<<call search_docs {\"query\": \"q\", \"filter\": {\"category\": \"api\", \"rogue\": true}}>>";
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(err.to_string().contains("unknown field 'filter.rogue'"));
}

#[test]
fn test_missing_required_field_root_and_nested() {
    let tools = test_tool_suite();

    // Missing root required field
    let no_query = "<<call search_docs {\"limit\": 10}>>";
    let err1 = decode_calls(no_query, &tools).unwrap_err();
    assert_eq!(err1.code(), "invalid_arguments");
    assert!(err1.to_string().contains("missing required field 'query'"));

    // Missing nested required field
    let no_cat = "<<call search_docs {\"query\": \"q\", \"filter\": {\"author\": \"alice\"}}>>";
    let err2 = decode_calls(no_cat, &tools).unwrap_err();
    assert_eq!(err2.code(), "invalid_arguments");
    assert!(
        err2.to_string()
            .contains("missing required field 'filter.category'")
    );
}

#[test]
fn test_enum_validation() {
    let tools = test_tool_suite();

    // Valid enum
    let valid = "<<call search_docs {\"query\": \"q\", \"filter\": {\"category\": \"api\"}}>>";
    assert!(decode_calls(valid, &tools).is_ok());

    // Invalid enum
    let invalid =
        "<<call search_docs {\"query\": \"q\", \"filter\": {\"category\": \"forbidden\"}}>>";
    let err = decode_calls(invalid, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(err.to_string().contains("invalid enum value 'forbidden'"));
}

#[test]
fn test_type_strictness_number_vs_integer() {
    let tools = test_tool_suite();

    // Float given for integer must fail
    let float_for_int = "<<call search_docs {\"query\": \"q\", \"limit\": 10.5}>>";
    let err1 = decode_calls(float_for_int, &tools).unwrap_err();
    assert_eq!(err1.code(), "invalid_arguments");
    assert!(err1.to_string().contains("must be an integer"));

    // Integer given for number must succeed
    let int_for_num = "<<call format_validator {\"ratio\": 42}>>";
    assert!(decode_calls(int_for_num, &tools).is_ok());

    // Float given for number must succeed
    let float_for_num = "<<call format_validator {\"ratio\": 3.1415}>>";
    assert!(decode_calls(float_for_num, &tools).is_ok());

    // String given for boolean must fail
    let str_for_bool = "<<call format_validator {\"is_active\": \"true\"}>>";
    let err2 = decode_calls(str_for_bool, &tools).unwrap_err();
    assert_eq!(err2.code(), "invalid_arguments");
    assert!(err2.to_string().contains("must be a boolean"));
}

#[test]
fn test_string_format_validations() {
    let tools = test_tool_suite();

    // Valid formats
    let valid_text = json!({
        "ts": "2026-10-05T14:30:00+05:30",
        "dt": "2026-10-05",
        "tm": "14:30:00.000Z",
        "email": "user.name+tag@example.co.uk",
        "link": "https://nasiko.org/docs/v1?ref=test#intro",
        "uuid": "f47ac10b-58cc-4372-a567-0e02b2c3d479"
    });
    let call_str = render_call("format_validator", &valid_text);
    assert!(decode_calls(&call_str, &tools).is_ok());

    // Invalid email
    let bad_email = "<<call format_validator {\"email\": \"not-an-email\"}>>";
    assert_eq!(
        decode_calls(bad_email, &tools).unwrap_err().code(),
        "invalid_arguments"
    );

    // Invalid uuid
    let bad_uuid = "<<call format_validator {\"uuid\": \"12345\"}>>";
    assert_eq!(
        decode_calls(bad_uuid, &tools).unwrap_err().code(),
        "invalid_arguments"
    );

    // Invalid date-time
    let bad_dt = "<<call format_validator {\"ts\": \"2026-99-99T99:99:99Z\"}>>";
    assert_eq!(
        decode_calls(bad_dt, &tools).unwrap_err().code(),
        "invalid_arguments"
    );
}

// =========================================================================
// 7. Unknown tool and malformed call syntax
// =========================================================================

#[test]
fn test_unknown_tool_rejection() {
    let tools = test_tool_suite();
    let text = "<<call execute_arbitrary_code {\"cmd\": \"rm -rf /\"}>>";
    let err = decode_calls(text, &tools).unwrap_err();
    assert_eq!(err.code(), "unknown_tool");
    assert!(matches!(err, Error::UnknownTool(name) if name == "execute_arbitrary_code"));
}

#[test]
fn test_malformed_syntax_rejection() {
    let tools = test_tool_suite();

    // Missing whitespace after <<call
    let no_space = "<<callsearch_docs {}>>";
    let err1 = decode_calls(no_space, &tools).unwrap_err();
    assert_eq!(err1.code(), "invalid_arguments");
    assert!(matches!(err1, Error::MalformedCall(_)));

    // Non-object arguments (array)
    let array_args = "<<call search_docs [\"query\", \"test\"]>>";
    let err2 = decode_calls(array_args, &tools).unwrap_err();
    assert_eq!(err2.code(), "invalid_arguments");
    assert!(matches!(err2, Error::MalformedCall(_)));

    // Bad closing token
    let bad_closing = "<<call search_docs {\"query\": \"test\"}>";
    let err3 = decode_calls(bad_closing, &tools).unwrap_err();
    assert_eq!(err3.code(), "invalid_arguments");
    assert!(matches!(err3, Error::UnterminatedCall));
}

// =========================================================================
// 8. Safety limits: Max calls, depth, size
// =========================================================================

#[test]
fn test_max_calls_limit() {
    let tools = test_tool_suite();
    // Emit 129 calls (limit is 128)
    let mut text = String::new();
    for _ in 0..129 {
        text.push_str("<<call no_params_tool {}>>\n");
    }
    let err = decode_calls(&text, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(matches!(err, Error::LimitExceeded(_)));
    assert!(err.to_string().contains("calls count exceeded 128"));
}

#[test]
fn test_excessive_depth_rejection() {
    let tools = test_tool_suite();
    // Nest 70 opening braces
    let mut open = String::new();
    let mut close = String::new();
    for _ in 0..70 {
        open.push_str("{\"k\":");
        close.push('}');
    }
    let text = format!("<<call search_docs {open}1{close}>>");
    let err = decode_calls(&text, &tools).unwrap_err();
    assert_eq!(err.code(), "invalid_arguments");
    assert!(matches!(err, Error::LimitExceeded(_)));
}

#[test]
fn test_poisoned_state_propagation() {
    let tools = test_tool_suite();
    let mut decoder = StreamDecoder::new(&tools).unwrap();

    // Feed an unknown tool which causes an error
    let push_res = decoder.push("<<call unknown_tool {}>>");
    assert!(push_res.is_err());

    // Subsequent push must immediately return the same error
    let next_push = decoder.push(" more text");
    assert!(next_push.is_err());

    // Finish must also return the poisoned error
    let finish_res = decoder.finish();
    assert!(finish_res.is_err());
}
