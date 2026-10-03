mod common;

use common::*;
use nasiko_tool_compact::{
    CompactError, DescriptionPolicy, EncodeOptions, ToolDef, decode_tools, encode_tools,
    encode_tools_with,
};
use serde_json::{Value, json};

#[test]
fn sample_tools_render_as_one_line_signatures() {
    let compact = encode_tools(&sample_tools()).unwrap();
    assert_eq!(
        compact.definitions(),
        "create_calendar_event(start:datetime, title:str, attendees?:[str] 'Attendee emails', \
duration_min?:int, visibility?:public|private) - Create an event in the user's calendar.\n\
send_email(body:str 'Plain-text body', subject:str 'Subject line', to:[str] 'Recipient emails', \
cc?:[str] 'CC emails') - Send an email from the user's account."
    );
}

#[test]
fn auto_policy_drops_only_descriptions_that_repeat_name_and_type() {
    let text = encode_tools(&[calendar()])
        .unwrap()
        .definitions()
        .to_string();
    // "Event title", "Start time, ISO 8601", "Duration in minutes" add nothing to the signature.
    assert!(!text.contains("Event title"));
    assert!(!text.contains("ISO 8601"));
    assert!(!text.contains("minutes"));
    // "Attendee emails" says the strings are emails: kept.
    assert!(text.contains("'Attendee emails'"));
}

#[test]
fn keep_policy_keeps_every_description() {
    let keep = EncodeOptions {
        descriptions: DescriptionPolicy::Keep,
    };
    let text = encode_tools_with(&[calendar()], keep).unwrap();
    for d in ["Event title", "Start time, ISO 8601", "Duration in minutes"] {
        assert!(text.definitions().contains(d), "{d}");
    }
}

#[test]
fn nested_schema_features_render_compactly() {
    let text = encode_tools(&[order()]).unwrap().definitions().to_string();
    assert_eq!(
        text,
        "place_order(customer:{id:uuid, tier?:free|pro|'it\\'s complicated'}, \
items:[{sku:str, qty?:int(1..99)=1}!] 'Line items; at least one', deliver_on?:date, \
discount?:num(0..0.5), express?:bool=false, meta?:obj, note?:str|null 'Gift message, or null', \
priority?:1|2|3)! - Place an order.\\nCharges the saved card."
    );
}

#[test]
fn field_order_does_not_depend_on_input_order() {
    let a = tool(
        "t",
        "d",
        json!({"type":"object","properties":{"b":{"type":"string"},"a":{"type":"integer"}},"required":["b"]}),
    );
    let b = tool(
        "t",
        "d",
        json!({"type":"object","required":["b"],"properties":{"a":{"type":"integer"},"b":{"type":"string"}}}),
    );
    assert_eq!(
        encode_tools(&[a]).unwrap().definitions(),
        encode_tools(&[b]).unwrap().definitions()
    );
}

#[test]
fn unsupported_schema_features_bypass_instead_of_losing_meaning() {
    let cases = [
        json!({"type":"object","properties":{"x":{"$ref":"#/defs/x"}}}),
        json!({"type":"object","properties":{"x":{"anyOf":[{"type":"string"},{"type":"integer"}]}}}),
        json!({"type":"object","properties":{"x":{"type":"string","pattern":"^a"}}}),
        json!({"type":"object","properties":{"x":{"type":"string","minLength":2}}}),
        json!({"type":"object","properties":{"x":{"type":["string","integer"]}}}),
        json!({"type":"object","properties":{"x":{"type":"string","format":"hostname"}}}),
        json!({"type":"object","properties":{"x":{"type":"array","items":[{"type":"string"}]}}}),
        json!({"type":"object","additionalProperties":{"type":"string"}}),
        json!({"type":"object","properties":{"x":{"type":"string"}},"required":["y"]}),
        json!({"type":"array","items":{"type":"string"}}),
    ];
    for params in cases {
        let err = encode_tools(&[tool("t", "d", params.clone())]).unwrap_err();
        assert!(
            matches!(err, CompactError::Unsupported { .. }),
            "{params}: {err:?}"
        );
    }
}

#[test]
fn bad_tool_names_are_rejected() {
    for name in ["", "has space", "dot.name", &"x".repeat(65)] {
        let t = ToolDef {
            name: name.into(),
            description: None,
            parameters: None,
        };
        assert_eq!(
            encode_tools(&[t]).unwrap_err().kind(),
            "invalid_tool",
            "{name}"
        );
    }
    assert_eq!(
        encode_tools(&[calendar(), calendar()]).unwrap_err().kind(),
        "invalid_tool"
    );
}

#[test]
fn tool_without_parameters_renders_empty_parens() {
    let t = ToolDef {
        name: "ping".into(),
        description: None,
        parameters: None,
    };
    assert_eq!(encode_tools(&[t]).unwrap().definitions(), "ping()");
}

#[test]
fn system_prompt_contains_definitions_and_call_format() {
    let compact = encode_tools(&sample_tools()).unwrap();
    let prompt = compact.system_prompt();
    assert!(prompt.contains(compact.definitions()));
    assert!(prompt.contains(r#"<<call NAME {"arg":value}>>"#));
}

/// The schema with every `required` list sorted: `required` is a set, and the crate emits it
/// in its own (deterministic) field order.
fn schema_of(t: &ToolDef) -> Value {
    fn sort_required(v: &mut Value) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Array(req)) = map.get_mut("required") {
                    req.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                }
                map.values_mut().for_each(sort_required);
            }
            Value::Array(items) => items.iter_mut().for_each(sort_required),
            _ => {}
        }
    }
    let mut schema = t.parameters.clone().unwrap();
    sort_required(&mut schema);
    schema
}

#[test]
fn decode_tools_restores_the_original_schemas_with_keep_policy() {
    let keep = EncodeOptions {
        descriptions: DescriptionPolicy::Keep,
    };
    let tools = all_tools();
    let decoded = decode_tools(&encode_tools_with(&tools, keep).unwrap()).unwrap();
    assert_eq!(decoded.len(), tools.len());
    for (orig, back) in tools.iter().zip(&decoded) {
        assert_eq!(orig.name, back.name);
        assert_eq!(orig.description, back.description);
        assert_eq!(schema_of(orig), schema_of(back), "{}", orig.name);
    }
}

#[test]
fn decode_tools_keeps_types_required_and_enums_with_auto_policy() {
    let decoded = decode_tools(&encode_tools(&sample_tools()).unwrap()).unwrap();
    let cal = schema_of(&decoded[0]);
    assert_eq!(cal["required"], json!(["start", "title"]));
    assert_eq!(cal["properties"]["start"]["format"], "date-time");
    assert_eq!(
        cal["properties"]["visibility"]["enum"],
        json!(["public", "private"])
    );
    assert_eq!(cal["properties"]["attendees"]["items"]["type"], "string");
    assert_eq!(
        cal["properties"]["attendees"]["description"],
        "Attendee emails"
    );
    assert_eq!(cal["properties"]["duration_min"]["type"], "integer");
}
