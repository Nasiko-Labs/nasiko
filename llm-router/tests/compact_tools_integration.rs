//! Integration tests for P1 Compact Tool Protocol in `nasiko-llm-router`.

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::compact_tools::{
    check_eligibility, prepare_chat_request, translate_chat_response, strip_call_markers,
    BypassReason, CompactPreparation,
};
use nasiko_llm_router::ir::chat::{
    ChatRequest, ChatResponse, Choice, FunctionDef, Message, ToolDef,
};
use serde_json::json;

fn calendar_tool() -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Event title" },
                    "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                    "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                    "visibility": { "type": "string", "enum": ["public", "private"] }
                },
                "required": ["title", "start"]
            })),
        },
        extra: serde_json::Map::new(),
    }
}

fn unsupported_tool() -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "bad_tool".to_string(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "val": {
                        "oneOf": [{ "type": "string" }, { "type": "integer" }]
                    }
                }
            })),
        },
        extra: serde_json::Map::new(),
    }
}

#[test]
fn test_eligibility_disabled_by_default() {
    let cfg = GatewayConfig::default();
    assert!(!cfg.compact_tools_enabled);

    let req = ChatRequest {
        model: Some("gpt-4o-mini".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(json!("Book a meeting")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }],
        tools: Some(vec![calendar_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };

    let res = check_eligibility(&req, &cfg);
    assert_eq!(res, Err(BypassReason::FeatureDisabled));
}

#[test]
fn test_eligibility_checks() {
    let mut cfg = GatewayConfig::default();
    cfg.compact_tools_enabled = true;

    // No tools
    let no_tools_req = ChatRequest {
        model: None,
        messages: vec![],
        tools: None,
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };
    assert_eq!(check_eligibility(&no_tools_req, &cfg), Err(BypassReason::NoTools));

    // Streaming
    let stream_req = ChatRequest {
        model: None,
        messages: vec![],
        tools: Some(vec![calendar_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        extra: serde_json::Map::new(),
    };
    assert_eq!(check_eligibility(&stream_req, &cfg), Err(BypassReason::StreamingUnsupported));

    // Unsupported tool choice (e.g. required)
    let tc_req = ChatRequest {
        model: None,
        messages: vec![],
        tools: Some(vec![calendar_tool()]),
        tool_choice: Some(json!("required")),
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };
    assert_eq!(check_eligibility(&tc_req, &cfg), Err(BypassReason::UnsupportedToolChoice));

    // Prior tool history in messages
    let history_req = ChatRequest {
        model: None,
        messages: vec![Message {
            role: "tool".to_string(),
            content: Some(json!("result")),
            name: None,
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            extra: serde_json::Map::new(),
        }],
        tools: Some(vec![calendar_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };
    assert_eq!(check_eligibility(&history_req, &cfg), Err(BypassReason::PriorToolHistory));
}

#[test]
fn test_prepare_chat_request_enabled_success() {
    let mut cfg = GatewayConfig::default();
    cfg.compact_tools_enabled = true;

    let mut req = ChatRequest {
        model: Some("gpt-4o-mini".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(json!("Schedule design review on Oct 5")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }],
        tools: Some(vec![calendar_tool()]),
        tool_choice: Some(json!("auto")),
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };

    let prep = prepare_chat_request(&mut req, &cfg);

    match prep {
        CompactPreparation::Compacted { original_tools } => {
            assert_eq!(original_tools.len(), 1);
            assert_eq!(original_tools[0].name, "create_calendar_event");
        }
        CompactPreparation::Native => panic!("expected CompactPreparation::Compacted"),
    }

    // Tools and tool_choice must be removed from outgoing request
    assert!(req.tools.is_none());
    assert!(req.tool_choice.is_none());

    // Messages must contain the compact instruction system message
    assert_eq!(req.messages.len(), 2);
    let system_msg = &req.messages[1];
    assert_eq!(system_msg.role, "system");
    let content = system_msg.content.as_ref().unwrap().as_str().unwrap();
    assert!(content.contains("CTP/1:"));
    assert!(content.contains("<<call NAME {JSON}>>"));
    assert!(content.contains("create_calendar_event"));
}

#[test]
fn test_prepare_chat_request_bypasses_unsupported_tool() {
    let mut cfg = GatewayConfig::default();
    cfg.compact_tools_enabled = true;

    let mut req = ChatRequest {
        model: Some("gpt-4o-mini".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(json!("Test")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }],
        tools: Some(vec![calendar_tool(), unsupported_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };

    let prep = prepare_chat_request(&mut req, &cfg);
    assert!(matches!(prep, CompactPreparation::Native));

    // Request is untouched
    assert!(req.tools.is_some());
    assert_eq!(req.messages.len(), 1);
}

#[test]
fn test_translate_chat_response_with_valid_calls() {
    let mut cfg = GatewayConfig::default();
    cfg.compact_tools_enabled = true;

    let mut req = ChatRequest {
        model: Some("gpt-4o-mini".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(json!("Schedule design review")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }],
        tools: Some(vec![calendar_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };

    let prep = prepare_chat_request(&mut req, &cfg);
    let original_tools = match prep {
        CompactPreparation::Compacted { original_tools } => original_tools,
        _ => panic!("expected compacted"),
    };

    let mut resp = ChatResponse {
        id: "chatcmpl-test".to_string(),
        object: "chat.completion".to_string(),
        created: Some(1727800000),
        model: "gpt-4o-mini".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content: Some(json!("I am booking your meeting now:\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>\nAll done!")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: None,
        extra: serde_json::Map::new(),
    };

    translate_chat_response(&mut resp, &original_tools).expect("translation must succeed");

    let choice = &resp.choices[0];
    assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));

    let calls = choice.message.tool_calls.as_ref().expect("tool_calls present");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].kind, "function");
    assert!(calls[0].id.starts_with("call_"));
    assert_eq!(calls[0].function.name, "create_calendar_event");
    assert_eq!(
        calls[0].function.arguments,
        "{\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\"}"
    );

    // Stripped message content
    let remaining_text = choice.message.content.as_ref().unwrap().as_str().unwrap();
    assert!(remaining_text.contains("I am booking your meeting now:"));
    assert!(remaining_text.contains("All done!"));
    assert!(!remaining_text.contains("<<call"));
}

#[test]
fn test_translate_chat_response_rejects_duplicate_key() {
    let mut cfg = GatewayConfig::default();
    cfg.compact_tools_enabled = true;

    let mut req = ChatRequest {
        model: Some("gpt-4o-mini".to_string()),
        messages: vec![],
        tools: Some(vec![calendar_tool()]),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: serde_json::Map::new(),
    };

    let prep = prepare_chat_request(&mut req, &cfg);
    let original_tools = match prep {
        CompactPreparation::Compacted { original_tools } => original_tools,
        _ => panic!("expected compacted"),
    };

    let mut resp = ChatResponse {
        id: "chatcmpl-test".to_string(),
        object: "chat.completion".to_string(),
        created: None,
        model: "gpt-4o-mini".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content: Some(json!("<<call create_calendar_event {\"title\":\"Review\",\"title\":\"Duplicate\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>")),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: None,
        extra: serde_json::Map::new(),
    };

    let res = translate_chat_response(&mut resp, &original_tools);
    assert!(res.is_err(), "must reject duplicate keys in model arguments");
}

#[test]
fn test_strip_call_markers_clean() {
    let text = "Here is the call: <<call foo {\"arg\": \">>\"}>> and done.";
    let stripped = strip_call_markers(text);
    assert_eq!(stripped, "Here is the call:  and done.");
}
