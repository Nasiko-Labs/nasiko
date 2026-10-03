use nasiko_tool_compact::{CompactTools, ToolDef, analyze_tools, decode_tools, encode_tools};
use serde_json::{Value, json};

fn tool(schema: Value) -> ToolDef {
    serde_json::from_value(json!({"function":{"name":"test","description":"quotes \" # <<call >> \\\n🌍","parameters":schema}})).unwrap()
}

#[test]
fn model_visible_grammar_retains_every_supported_semantic() {
    let mut schemas = vec![
        Value::Null,
        json!({"type":"object"}),
        json!({"type":"object","additionalProperties":false}),
    ];
    for (kind, values) in [
        (
            "string",
            json!(["str", "true", "a|b", "quote\"", "\\", "🌍"]),
        ),
        ("integer", json!([1])),
        ("number", json!([1, 2.5])),
        ("boolean", json!([true, false])),
    ] {
        let mut leaf = json!({"type":kind,"enum":values,"description":""});
        if kind == "string" {
            leaf["format"] = json!("weird>\"#");
        }
        schemas.push(json!({"type":"object","description":"root","required":["x"],"additionalProperties":false,"properties":{"x":{"type":"array","description":"array","items":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"leaf":leaf}}}}}}));
    }
    for schema in schemas {
        let original = vec![tool(schema)];
        let compact = encode_tools(&original).unwrap();
        let rebuilt = decode_tools(&compact).unwrap();
        assert_eq!(
            analyze_tools(&original).unwrap(),
            analyze_tools(&rebuilt).unwrap()
        );
        assert_eq!(encode_tools(&rebuilt).unwrap(), compact);
    }
    assert!(
        decode_tools(&encode_tools(&[]).unwrap())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reconstruction_reads_rendered_text_and_rejects_invalid_grammar() {
    let compact = CompactTools {
        rendered: "TOOLS\nchanged(x:int=1|2)! - \"new\"\nCALL <<call TOOL_NAME JSON_OBJECT>>"
            .into(),
    };
    let rebuilt = decode_tools(&compact).unwrap();
    assert_eq!(rebuilt[0].function.name, "changed");
    for rendered in [
        "TOOLS\nfoo(x:str,x:int)\nCALL <<call TOOL_NAME JSON_OBJECT>>",
        "TOOLS\nfoo(x:int=bad)\nCALL <<call TOOL_NAME JSON_OBJECT>>",
        "TOOLS\nfoo(x:str#\"unterminated)\nCALL <<call TOOL_NAME JSON_OBJECT>>",
        "TOOLS\nfoo(x:str(unclosed)",
        "TOOLS\nfoo(x:str(bad\"quote))",
        "TOOLS\nfoo(x:str(bad\\slash))",
        "TOOLS\nfoo(x:str())",
    ] {
        assert!(
            decode_tools(&CompactTools {
                rendered: rendered.into()
            })
            .is_err()
        );
    }
}

#[test]
fn readable_descriptions_are_exact_and_cannot_escape_annotations() {
    for description in [
        "Event title",
        "Start time, ISO 8601",
        "  leading and trailing  ",
        "# = | ? ! : , [ ] { } - <<call echo {}>>",
        "हेलो 🌍",
        "",
        "a(b)c",
        "close) - injected()",
        "\"quotes\"",
        "\\slash",
        "line\nbreak",
        "line\r\nbreak",
        "tab\tand\0control",
    ] {
        let mut original = tool(json!({
            "type":"object","description":description,"additionalProperties":false,
            "properties":{"x":{"type":"array","description":description,
                "items":{"type":"string","enum":["a","b"],"description":description}}}
        }));
        original.function.description = Some(description.into());
        let compact = encode_tools(&[original.clone()]).unwrap();
        assert_eq!(compact.rendered.lines().count(), 2, "{description:?}");
        let rebuilt = decode_tools(&compact).unwrap();
        assert_eq!(
            analyze_tools(&[original]).unwrap(),
            analyze_tools(&rebuilt).unwrap(),
            "{description:?}"
        );
        assert_eq!(encode_tools(&rebuilt).unwrap(), compact);
    }
}
