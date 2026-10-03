use nasiko_tool_compact::{
    ToolCall, ToolDef, decode_calls, encode_tools, render_call, validate_arguments,
};
use serde_json::json;

#[test]
fn encoder_render_decode_validate_pipeline() {
    let tools = vec![ToolDef {
        name: "schedule_meeting".into(),
        description: Some("Schedule a meeting.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "attendees": {"type": "array", "items": {"type": "string"}},
                "duration_minutes": {"type": "integer"}
            },
            "required": ["title", "attendees"]
        })),
    }];
    let expected = ToolCall {
        name: "schedule_meeting".into(),
        arguments: json!({
            "title": "Planning",
            "attendees": ["dev@example.com"],
            "duration_minutes": 30
        }),
    };

    let compact = encode_tools(&tools).expect("Group A encoder must compact the tool");
    assert!(compact.definitions.contains("schedule_meeting("));

    let rendered = render_call(&expected);
    let decoded = decode_calls(&rendered, &tools).expect("Group B decoder must decode the call");
    assert_eq!(decoded, vec![expected.clone()]);

    validate_arguments(
        tools[0]
            .parameters
            .as_ref()
            .expect("test tool has a schema"),
        &decoded[0].arguments,
    )
    .expect("Group B validator must accept the decoded arguments");
}
