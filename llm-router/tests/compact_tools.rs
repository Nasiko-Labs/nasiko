use nasiko_llm_router::{
    compact_tools::{Bypass, prepare_request, supports_wire_tools},
    ir::{ChatRequest, ChatResponse},
};
use serde_json::{Value, json};

fn request() -> ChatRequest {
    let dataset: Value =
        serde_json::from_str(include_str!("fixtures/compact-tools-eval-v1.json")).unwrap();
    serde_json::from_value(json!({"messages":[{"role":"system","content":"Existing instruction"},{"role":"user","content":"Book a meeting"}],"tools":dataset["tools"]})).unwrap()
}

fn response(text: &str) -> ChatResponse {
    serde_json::from_value(json!({"id":"completion","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}})).unwrap()
}

#[test]
fn compact_request_preserves_messages_and_restores_client_contract() {
    let mut request = request();
    let original = request.messages.clone();
    let prepared = prepare_request(&mut request).unwrap();
    assert!(request.tools.is_none());
    assert_eq!(
        serde_json::to_value(&request.messages[0]).unwrap(),
        serde_json::to_value(&original[0]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&request.messages[2]).unwrap(),
        serde_json::to_value(&original[1]).unwrap()
    );
    let response = prepared.restore(response("Before <<call create_calendar_event {\"title\":\"Review\",\"start\":\"2026-10-05T15:00:00+05:30\"}>> after")).unwrap();
    let choice = &response.choices[0];
    assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
    assert_eq!(choice.message.content, Some(json!("Before  after")));
    let calls = choice.message.tool_calls.as_ref().unwrap();
    assert!(calls[0].id.starts_with("call_"));
    assert_eq!(calls[0].kind, "function");
    assert_eq!(
        serde_json::from_str::<Value>(&calls[0].function.arguments).unwrap()["title"],
        "Review"
    );
    assert_eq!(response.usage.unwrap().prompt_tokens, Some(10));
}

#[test]
fn malformed_and_truncated_provider_responses_do_not_leak_calls() {
    for text in [
        "<<call nonexistent {}>>",
        "<<call create_calendar_event {}>>",
        "<<call create_calendar_event {",
        "<<call send_email {\"to\":[\"a@b.com\"],\"subject\":\"Hi\",\"body\":\"x\"}>> <<call nonexistent {}>>",
    ] {
        let mut req = request();
        assert!(
            prepare_request(&mut req)
                .unwrap()
                .restore(response(text))
                .is_err()
        );
    }
    let mut req = request();
    let mut truncated = response("Plain response");
    truncated.choices[0].finish_reason = Some("length".into());
    assert!(
        prepare_request(&mut req)
            .unwrap()
            .restore(truncated)
            .is_err()
    );
}

#[test]
fn all_bypasses_are_transactional() {
    let base = serde_json::to_value(request()).unwrap();
    for (key, replacement, reason) in [
        ("stream", json!(true), Bypass::Streaming),
        ("tool_choice", json!("required"), Bypass::ToolChoice),
        (
            "response_format",
            json!({"type":"json_object"}),
            Bypass::RequestOptions,
        ),
        ("stop", json!([">>"]), Bypass::RequestOptions),
        ("parallel_tool_calls", json!(false), Bypass::RequestOptions),
        ("n", json!(2), Bypass::RequestOptions),
    ] {
        let mut body = base.clone();
        body[key] = replacement;
        let mut req: ChatRequest = serde_json::from_value(body).unwrap();
        let before = serde_json::to_vec(&req).unwrap();
        assert_eq!(prepare_request(&mut req).err(), Some(reason));
        assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    }
}

#[test]
fn provider_strict_and_unknown_function_fields_are_bypassed_before_ir_parsing() {
    let mut body = serde_json::to_value(request()).unwrap();
    assert!(supports_wire_tools(&body));
    body["tools"][0]["function"]["strict"] = json!(true);
    assert!(!supports_wire_tools(&body));
}

#[test]
fn tool_history_and_unsupported_schemas_remain_native() {
    let mut req = request();
    req.messages.push(
        serde_json::from_value(json!({"role":"tool","tool_call_id":"x","content":"done"})).unwrap(),
    );
    let before = serde_json::to_vec(&req).unwrap();
    assert_eq!(prepare_request(&mut req).err(), Some(Bypass::History));
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
    let mut req = request();
    req.tools.as_mut().unwrap()[0].function.parameters =
        Some(json!({"$ref":"https://example.com/schema"}));
    let before = serde_json::to_vec(&req).unwrap();
    assert_eq!(
        prepare_request(&mut req).err(),
        Some(Bypass::UnsupportedSchema)
    );
    assert_eq!(serde_json::to_vec(&req).unwrap(), before);
}
