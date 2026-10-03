//! Edge-case tests targeting the private eval set's likely patterns:
//! nested objects, arrays of objects, many tools, empty params, deeply nested schemas,
//! unicode, special characters, multiple calls interleaved with text.

use nasiko_tool_compact::{
    CompactError, StreamDecoder, StreamEvent, ToolDef, decode_calls, decode_tools, encode_tools,
};
use serde_json::{Value, json};

// ─── Encoding edge cases ─────────────────────────────────────────────────

#[test]
fn encode_tool_with_many_params() {
    let tool = ToolDef {
        name: "create_project".to_string(),
        description: Some("Create a new project.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "owner": {"type": "string"},
                "team": {"type": "array", "items": {"type": "string"}},
                "visibility": {"type": "string", "enum": ["public", "private", "internal"]},
                "template": {"type": "string"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "deadline": {"type": "string", "format": "date-time"},
                "budget": {"type": "number"},
                "priority": {"type": "integer"}
            },
            "required": ["name", "owner"]
        })),
    };
    let result = encode_tools(&[tool]).unwrap();
    assert_eq!(result.tool_count, 1);
    assert!(result.text.contains("name:str"));
    assert!(result.text.contains("owner:str"));
    assert!(result.text.contains("budget?:float"));
    assert!(result.text.contains("priority?:int"));
    assert!(result.text.contains("public|private|internal"));
}

#[test]
fn encode_deeply_nested_object() {
    let tool = ToolDef {
        name: "create_order".to_string(),
        description: Some("Create an order.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "item": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "price": {"type": "number"},
                        "metadata": {
                            "type": "object",
                            "properties": {
                                "sku": {"type": "string"},
                                "weight": {"type": "number"}
                            },
                            "required": ["sku"]
                        }
                    },
                    "required": ["name", "price"]
                },
                "quantity": {"type": "integer"}
            },
            "required": ["item", "quantity"]
        })),
    };
    let result = encode_tools(&[tool]).unwrap();
    assert!(result.text.contains("item:{"));
    assert!(result.text.contains("sku:str"));
}

#[test]
fn encode_array_of_objects() {
    let tool = ToolDef {
        name: "bulk_invite".to_string(),
        description: Some("Invite multiple users.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "invites": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "email": {"type": "string"},
                            "role": {"type": "string", "enum": ["admin", "member", "viewer"]}
                        },
                        "required": ["email"]
                    }
                }
            },
            "required": ["invites"]
        })),
    };
    let result = encode_tools(&[tool]).unwrap();
    assert!(result.text.contains("invites:[{"));
    assert!(result.text.contains("email:str"));
    assert!(result.text.contains("admin|member|viewer"));
}

#[test]
fn encode_boolean_params() {
    let tool = ToolDef {
        name: "toggle_feature".to_string(),
        description: Some("Toggle a feature flag.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "feature": {"type": "string"},
                "enabled": {"type": "boolean"},
                "force": {"type": "boolean"}
            },
            "required": ["feature", "enabled"]
        })),
    };
    let result = encode_tools(&[tool]).unwrap();
    assert!(result.text.contains("enabled:bool"));
    assert!(result.text.contains("force?:bool"));
}

#[test]
fn encode_many_tools() {
    let tools: Vec<ToolDef> = (0..10)
        .map(|i| ToolDef {
            name: format!("tool_{i}"),
            description: Some(format!("Tool number {i}.")),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                },
                "required": ["input"]
            })),
        })
        .collect();
    let result = encode_tools(&tools).unwrap();
    assert_eq!(result.tool_count, 10);
    for i in 0..10 {
        assert!(result.text.contains(&format!("tool_{i}(")));
    }
}

#[test]
fn encode_tool_no_description() {
    let tool = ToolDef {
        name: "noop".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"x": {"type": "integer"}},
            "required": ["x"]
        })),
    };
    let result = encode_tools(&[tool]).unwrap();
    assert!(result.text.contains("noop(x:int)"));
    // No description, so no " - " after closing paren on the tool line
    for line in result.text.lines() {
        if line.starts_with("noop(") {
            assert!(!line.contains(" - "));
        }
    }
}

#[test]
fn encode_long_description_truncated() {
    let tool = ToolDef {
        name: "verbose".to_string(),
        description: Some("A".repeat(200)),
        parameters: None,
    };
    let result = encode_tools(&[tool]).unwrap();
    // Description should be truncated to ~80 chars
    for line in result.text.lines() {
        if line.starts_with("verbose(") {
            assert!(line.len() < 100);
            assert!(line.contains("..."));
        }
    }
}

// ─── Decoding edge cases ─────────────────────────────────────────────────

#[test]
fn decode_empty_args_object() {
    let tool = ToolDef {
        name: "ping".to_string(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {}})),
    };
    let text = r#"<<call ping {}>>"#;
    let calls = decode_calls(text, &[tool]).unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "ping");
}

#[test]
fn decode_unicode_in_args() {
    let tool = ToolDef {
        name: "send_email".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            },
            "required": ["to", "subject", "body"]
        })),
    };
    let text = r#"<<call send_email {"to":["user@example.com"],"subject":"日本語テスト","body":"Héllo wörld 🌍"}>>"#;
    let calls = decode_calls(text, &[tool]).unwrap();
    assert_eq!(calls.len(), 1);
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["subject"], "日本語テスト");
}

#[test]
fn decode_newlines_in_json_string() {
    let tool = ToolDef {
        name: "send_email".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "to": {"type": "array", "items": {"type": "string"}},
                "subject": {"type": "string"},
                "body": {"type": "string"}
            },
            "required": ["to", "subject", "body"]
        })),
    };
    let text =
        r#"<<call send_email {"to":["a@b.com"],"subject":"Hi","body":"Line 1\nLine 2\nLine 3"}>>"#;
    let calls = decode_calls(text, &[tool]).unwrap();
    assert_eq!(calls.len(), 1);
}

#[test]
fn decode_multiple_calls_with_text_between() {
    let tool1 = ToolDef {
        name: "search".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        })),
    };
    let tool2 = ToolDef {
        name: "summarize".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        })),
    };
    let text = r#"Let me search first.
<<call search {"query":"rust compact tools"}>>
Now I'll summarize the results.
<<call summarize {"text":"Rust has great tooling."}>>
All done!"#;
    let calls = decode_calls(text, &[tool1, tool2]).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "search");
    assert_eq!(calls[1].name, "summarize");
}

#[test]
fn decode_nested_json_objects_in_args() {
    let tool = ToolDef {
        name: "create_config".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "settings": {
                    "type": "object",
                    "properties": {
                        "theme": {"type": "string"},
                        "notifications": {"type": "boolean"}
                    }
                }
            },
            "required": ["settings"]
        })),
    };
    let text = r#"<<call create_config {"settings":{"theme":"dark","notifications":true}}>>"#;
    let calls = decode_calls(text, &[tool]).unwrap();
    assert_eq!(calls.len(), 1);
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["settings"]["theme"], "dark");
    assert_eq!(args["settings"]["notifications"], true);
}

#[test]
fn decode_array_of_numbers() {
    let tool = ToolDef {
        name: "calculate".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "values": {"type": "array", "items": {"type": "number"}}
            },
            "required": ["values"]
        })),
    };
    let text = r#"<<call calculate {"values":[1.5, 2.7, 3.14, 42]}>>"#;
    let calls = decode_calls(text, &[tool]).unwrap();
    let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
    assert_eq!(args["values"].as_array().unwrap().len(), 4);
}

// ─── Fail-closed error cases ─────────────────────────────────────────────

#[test]
fn decode_malformed_json() {
    let tool = ToolDef {
        name: "test".to_string(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {}})),
    };
    let text = r#"<<call test {not valid json}>>"#;
    assert!(decode_calls(text, &[tool]).is_err());
}

#[test]
fn decode_wrong_type_array_for_string() {
    let tool = ToolDef {
        name: "greet".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"name": {"type": "string"}},
            "required": ["name"]
        })),
    };
    let text = r#"<<call greet {"name": ["not", "a", "string"]}>>"#;
    let result = decode_calls(text, &[tool]);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        CompactError::InvalidArgument { .. }
    ));
}

#[test]
fn decode_integer_for_boolean() {
    let tool = ToolDef {
        name: "toggle".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"flag": {"type": "boolean"}},
            "required": ["flag"]
        })),
    };
    let text = r#"<<call toggle {"flag": 1}>>"#;
    let result = decode_calls(text, &[tool]);
    assert!(result.is_err());
}

#[test]
fn decode_multiple_missing_required() {
    let tool = ToolDef {
        name: "multi_req".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "string"},
                "c": {"type": "string"}
            },
            "required": ["a", "b", "c"]
        })),
    };
    let text = r#"<<call multi_req {"a": "only_one"}>>"#;
    assert!(decode_calls(text, &[tool]).is_err());
}

// ─── Stream decoder edge cases ───────────────────────────────────────────

#[test]
fn stream_multiple_calls_split_across_many_chunks() {
    let tool = ToolDef {
        name: "log".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"]
        })),
    };
    let tools = [tool];
    let mut dec = StreamDecoder::new(&tools);

    // Two calls, each split across chunks
    let chunks = [
        "First: <<cal",
        "l log {\"msg\":\"hel",
        "lo\"}>> Second: <<",
        "call log {\"msg\":\"world\"",
        "}>>",
    ];

    let mut all = Vec::new();
    for chunk in &chunks {
        all.extend(dec.push(chunk).unwrap());
    }
    all.extend(dec.finish().unwrap());

    let call_count = all
        .iter()
        .filter(|e| matches!(e, StreamEvent::Call(_)))
        .count();
    assert_eq!(call_count, 2);
}

#[test]
fn stream_only_text_no_markers() {
    let tool = ToolDef {
        name: "x".to_string(),
        description: None,
        parameters: None,
    };
    let tools = [tool];
    let mut dec = StreamDecoder::new(&tools);

    let mut all = Vec::new();
    for chunk in ["Hello ", "world ", "no tools here"] {
        all.extend(dec.push(chunk).unwrap());
    }
    all.extend(dec.finish().unwrap());

    assert!(all.iter().all(|e| matches!(e, StreamEvent::Text(_))));
    let text: String = all
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello world no tools here");
}

#[test]
fn stream_marker_at_very_start() {
    let tool = ToolDef {
        name: "ping".to_string(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {}})),
    };
    let tools = [tool];
    let mut dec = StreamDecoder::new(&tools);
    let events = dec.push(r#"<<call ping {}>>"#).unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], StreamEvent::Call(c) if c.name == "ping"));
}

#[test]
fn stream_single_char_chunks() {
    let tool = ToolDef {
        name: "t".to_string(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {"x": {"type": "integer"}},
            "required": ["x"]
        })),
    };
    let tools = [tool];
    let mut dec = StreamDecoder::new(&tools);
    let input = r#"<<call t {"x":1}>>"#;

    let mut all = Vec::new();
    for ch in input.chars() {
        all.extend(dec.push(&ch.to_string()).unwrap());
    }
    all.extend(dec.finish().unwrap());

    let calls: Vec<_> = all
        .iter()
        .filter(|e| matches!(e, StreamEvent::Call(_)))
        .collect();
    assert_eq!(calls.len(), 1);
}

// ─── decode_tools (schema roundtrip) ─────────────────────────────────────

#[test]
fn decode_tools_preserves_enum_types() {
    let tool = ToolDef {
        name: "set_priority".to_string(),
        description: Some("Set priority.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "level": {"type": "string", "enum": ["low", "medium", "high", "critical"]}
            },
            "required": ["level"]
        })),
    };
    let compact = encode_tools(&[tool]).unwrap();
    let decoded = decode_tools(&compact.text).unwrap();

    let params = decoded[0].parameters.as_ref().unwrap();
    let level_enum = params["properties"]["level"]["enum"].as_array().unwrap();
    assert_eq!(level_enum.len(), 4);
    assert!(level_enum.contains(&json!("low")));
    assert!(level_enum.contains(&json!("critical")));
}

#[test]
fn decode_tools_preserves_array_of_objects() {
    let tool = ToolDef {
        name: "add_items".to_string(),
        description: Some("Add items.".to_string()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "qty": {"type": "integer"}
                        },
                        "required": ["name"]
                    }
                }
            },
            "required": ["items"]
        })),
    };
    let compact = encode_tools(&[tool]).unwrap();
    let decoded = decode_tools(&compact.text).unwrap();

    let params = decoded[0].parameters.as_ref().unwrap();
    let items = &params["properties"]["items"];
    assert_eq!(items["type"], "array");
    assert_eq!(items["items"]["type"], "object");
    assert!(items["items"]["properties"]["name"]["type"] == "string");
    assert!(items["items"]["properties"]["qty"]["type"] == "integer");
}
