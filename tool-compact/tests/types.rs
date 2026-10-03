use nasiko_tool_compact::ToolDef;
use serde_json::json;

#[test]
fn unknown_metadata_survives_deserialization_for_preflight() {
    let original =
        json!({"type":"function","vendor":true,"function":{"name":"ping","strict":true}});
    let tool: ToolDef = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(tool).unwrap(), original);
}
