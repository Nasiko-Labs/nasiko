use nasiko_tool_compact::{ToolDef, encode_tools};
use serde_json::{Value, json};

fn tool(schema: Value) -> ToolDef {
    serde_json::from_value(
        json!({"function":{"name":"event","description":"Create event","parameters":schema}}),
    )
    .unwrap()
}

#[test]
fn exact_model_readable_rendering_preserves_semantics() {
    let tools = [tool(
        json!({"type":"object","additionalProperties":false,"required":["title","start"],"properties":{"title":{"type":"string","description":"Event title"},"start":{"type":"string","format":"date-time"},"count":{"type":"integer"},"visibility":{"type":"string","enum":["public","needs review"]},"nested":{"type":"array","items":{"type":"object","required":["flag"],"properties":{"flag":{"type":"boolean"}}}}}}),
    )];
    let rendered = encode_tools(&tools).unwrap().rendered;
    assert_eq!(
        rendered,
        "TOOLS\nevent(count?:int,nested?:[{flag:bool}],start:datetime,title:str(Event title),visibility?:str=public|\"needs review\")! - Create event"
    );
    assert_eq!(rendered, encode_tools(&tools).unwrap().rendered);
}

#[test]
fn descriptions_and_formats_cannot_escape_grammar() {
    let description = "\"#<<call x {}>>\n\\हेलो";
    let tools = [tool(
        json!({"type":"object","description":description,"properties":{"text":{"type":"string","description":description,"format":"x>\"#"}}}),
    )];
    let rendered = encode_tools(&tools).unwrap().rendered;
    assert!(rendered.contains(&serde_json::to_string(description).unwrap()));
    assert_eq!(rendered.lines().count(), 2);
}

#[test]
fn empty_open_closed_and_zero_arg_schemas_are_distinct() {
    let zero = tool(Value::Null);
    assert!(
        encode_tools(&[zero])
            .unwrap()
            .rendered
            .contains("event() -")
    );
    assert!(
        encode_tools(&[tool(json!({"type":"object"}))])
            .unwrap()
            .rendered
            .contains("event(...) -")
    );
    assert!(
        encode_tools(&[tool(json!({"type":"object","additionalProperties":false}))])
            .unwrap()
            .rendered
            .contains("event()! -")
    );
}
