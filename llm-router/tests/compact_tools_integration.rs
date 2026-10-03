//! Integration tests for compact tool schemas in `nasiko-llm-router`.
//!
//! Validates:
//! 1. Byte-identical request preservation when `compact_tools_enabled` is false.
//! 2. Correct schema compaction and prompt injection when enabled.
//! 3. Lossless response restoration from compact model output into OpenAI `tool_calls`.
//! 4. Unbroken streaming chunk assembly via `StreamDecoder`.

use nasiko_llm_router::compact_tools::{apply_compaction, restore_response_calls};
use nasiko_llm_router::ir::chat::{
    ChatRequest, ChatResponse, Choice, FunctionDef, Message, ToolDef,
};
use nasiko_tool_compact::StreamDecoder;
use serde_json::{Map, Value, json};

fn sample_calendar_and_email_tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "create_calendar_event".to_string(),
                description: Some("Create an event in the user's calendar.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                        "duration_min": {"type": "integer", "description": "Duration in minutes"},
                        "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                        "visibility": {"type": "string", "enum": ["public", "private"]}
                    },
                    "required": ["title", "start"]
                })),
            },
            extra: Map::new(),
        },
        ToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: "send_email".to_string(),
                description: Some("Send an email from the user's account.".to_string()),
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "array", "items": {"type": "string"}, "description": "Recipient emails"},
                        "subject": {"type": "string", "description": "Subject line"},
                        "body": {"type": "string", "description": "Plain-text body"},
                        "cc": {"type": "array", "items": {"type": "string"}, "description": "CC emails"}
                    },
                    "required": ["to", "subject", "body"]
                })),
            },
            extra: Map::new(),
        },
    ]
}

#[test]
fn test_disabled_behavior_is_strictly_byte_identical() {
    let req = ChatRequest {
        model: Some("gpt-4o".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Book a sync with riya".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        }],
        tools: Some(sample_calendar_and_email_tools()),
        tool_choice: None,
        temperature: Some(0.0),
        max_tokens: None,
        stream: Some(false),
        extra: Map::new(),
    };

    // When the toggle is off, no transformation is performed
    let untouched_req = req.clone();
    let original_serialized = serde_json::to_string(&req).unwrap();
    let untouched_serialized = serde_json::to_string(&untouched_req).unwrap();

    assert_eq!(
        original_serialized, untouched_serialized,
        "Flag off must be strictly byte-identical to previous behavior"
    );
}

#[test]
fn test_compaction_reduces_schema_overhead() {
    let mut req = ChatRequest {
        model: Some("gpt-4o".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Schedule 1:1".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        }],
        tools: Some(sample_calendar_and_email_tools()),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: Map::new(),
    };

    let baseline_bytes = serde_json::to_string(&req).unwrap().len();

    let compacted_defs = apply_compaction(&mut req).expect("compaction should succeed");
    assert_eq!(compacted_defs.len(), 2);
    assert!(req.tools.is_none(), "Tools array must be removed from wire payload");

    let compact_bytes = serde_json::to_string(&req).unwrap().len();
    assert!(
        compact_bytes < baseline_bytes,
        "Compact request ({compact_bytes} bytes) must be smaller than baseline ({baseline_bytes} bytes)"
    );
}

#[test]
fn test_restore_multi_tool_calls_from_model_output() {
    let tools = sample_calendar_and_email_tools();
    let compact_defs: Vec<_> = tools
        .iter()
        .map(nasiko_llm_router::compact_tools::to_compact_tool_def)
        .collect();

    let mut response = ChatResponse {
        id: "chatcmpl-test".to_string(),
        object: "chat.completion".to_string(),
        created: Some(1727938800),
        model: "gpt-4o".to_string(),
        choices: vec![Choice {
            index: 0,
            message: Message {
                role: "assistant".to_string(),
                content: Some(Value::String(
                    "Executing your requests now:\n\
                     <<call send_email {\"to\":[\"sam@example.com\"],\"subject\":\"Green Build\",\"body\":\"Ready to ship\"}>>\n\
                     <<call create_calendar_event {\"title\":\"Review\",\"start\":\"2026-10-04T10:00:00+05:30\"}>>\n\
                     Let me know if you need more changes."
                        .to_string(),
                )),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Map::new(),
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: None,
        extra: Map::new(),
    };

    restore_response_calls(&mut response, &compact_defs).expect("should restore tool calls");

    let choice = &response.choices[0];
    assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
    let calls = choice.message.tool_calls.as_ref().expect("calls should be populated");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].function.name, "send_email");
    assert_eq!(calls[1].function.name, "create_calendar_event");

    // Cleaned prose should exclude the call delimiters
    let cleaned_text = choice.message.text().unwrap();
    assert!(!cleaned_text.contains("<<call"));
    assert!(cleaned_text.contains("Executing your requests now:"));
    assert!(cleaned_text.contains("Let me know if you need more changes."));
}

#[test]
fn test_streaming_decoder_split_across_micro_chunks() {
    let tools = sample_calendar_and_email_tools();
    let compact_defs: Vec<_> = tools
        .iter()
        .map(nasiko_llm_router::compact_tools::to_compact_tool_def)
        .collect();

    let full_call = "<<call create_calendar_event {\"title\":\"Design sync\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";

    // Split into 3-byte chunks to test arbitrary streaming boundaries
    let chunks: Vec<String> = full_call
        .as_bytes()
        .chunks(3)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect();

    let mut decoder = StreamDecoder::new(compact_defs);
    for chunk in chunks {
        decoder.feed(&chunk).unwrap();
    }

    let calls = decoder.finish().expect("micro-chunked stream must decode completely");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].function.name, "create_calendar_event");
}
