use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef};
use serde_json::Value;
use std::fs;

#[test]
fn test_live_evaluation_dataset_sample() {
    let eval_data = fs::read_to_string("/tmp/compact-tools-eval.json")
        .expect("Evaluation dataset /tmp/compact-tools-eval.json should exist");
    
    let json: Value = serde_json::from_str(&eval_data).unwrap();
    let tools_raw = json["tools"].as_array().unwrap();
    let tools: Vec<ToolDef> = serde_json::from_value(Value::Array(tools_raw.clone())).unwrap();

    // 1. Test encoding of the tools
    let compact = encode_tools(&tools).expect("Encoding should succeed");
    println!("\n=== COMPACT TOOL DEFINITIONS ===\n{}", compact.definitions);
    assert!(compact.definitions.contains("create_calendar_event("));
    assert!(compact.definitions.contains("send_email("));

    // 2. Test Case ct-001 (Single call)
    let call_ct001 = r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#;
    let decoded_001 = decode_calls(call_ct001, &tools).expect("ct-001 should decode successfully");
    assert_eq!(decoded_001.len(), 1);
    assert_eq!(decoded_001[0].function.name, "create_calendar_event");
    let args_001: Value = serde_json::from_str(&decoded_001[0].function.arguments).unwrap();
    assert_eq!(args_001["title"], "Design review");
    assert_eq!(args_001["attendees"][0], "riya@example.com");

    // 3. Test Case ct-002 (Multiple calls in single message)
    let call_ct002 = r#"<<call send_email {"to":["sam@example.com"],"subject":"Build status","body":"The build is green."}>> <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","duration_min":30,"visibility":"private"}>>"#;
    let decoded_002 = decode_calls(call_ct002, &tools).expect("ct-002 should decode successfully");
    assert_eq!(decoded_002.len(), 2);
    assert_eq!(decoded_002[0].function.name, "send_email");
    assert_eq!(decoded_002[1].function.name, "create_calendar_event");

    // 4. Test Case ct-003 (Plain text, zero calls)
    let plain_text = "The weather today in Bengaluru is 28°C and partly cloudy.";
    let decoded_003 = decode_calls(plain_text, &tools).expect("ct-003 should decode successfully");
    assert_eq!(decoded_003.len(), 0);

    // 5. Test Case dc-004 (Unknown tool -> fail-closed error)
    let unknown_tool_call = r#"<<call delete_everything {}>>"#;
    let err_004 = decode_calls(unknown_tool_call, &tools).unwrap_err();
    assert_eq!(err_004.eval_error_kind(), "unknown_tool");

    // 6. Test Case dc-005 (Missing required field + invalid enum -> fail-closed error)
    let invalid_call = r#"<<call create_calendar_event {"start":"2026-10-05T15:00:00+05:30","visibility":"secret"}>>"#;
    let err_005 = decode_calls(invalid_call, &tools).unwrap_err();
    assert_eq!(err_005.eval_error_kind(), "invalid_arguments");

    println!("\n✅ ALL EVALUATION TEST CASES PASSED SUCCESSFULLY!");
}
