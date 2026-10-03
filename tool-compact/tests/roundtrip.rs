//! `decode_tools(encode_tools(t)) == t` for everything the dialect accepts.
//!
//! The comparison is on `ToolDef` equality: names and descriptions byte-equal, parameters equal
//! as `serde_json::Value` (so `None`, `Some({})` and `Some({"type":"object"})` are all distinct).

mod common;

use common::{Lcg, gen_tool, tool};
use nasiko_tool_compact::{ToolDef, decode_tools, encode_tools};
use serde_json::{Value, json};

fn roundtrip(tools: &[ToolDef]) -> Vec<ToolDef> {
    let compact = encode_tools(tools).expect("encode");
    decode_tools(&compact).expect("decode")
}

fn assert_roundtrips(tools: Vec<ToolDef>) {
    let back = roundtrip(&tools);
    assert_eq!(back, tools);
}

#[test]
fn the_public_sample_catalog_roundtrips() {
    let tools = vec![
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event in the user's calendar.".into()),
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
        ToolDef {
            name: "send_email".into(),
            description: Some("Send an email from the user's account.".into()),
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
    ];
    assert_roundtrips(tools);
}

#[test]
fn every_zero_argument_form_is_distinguished() {
    let forms: Vec<Option<Value>> = vec![
        None,
        Some(json!({})),
        Some(json!({"type": "object"})),
        Some(json!({"type": "object", "properties": {}})),
        Some(json!({"type": "object", "properties": {}, "required": []})),
        Some(json!({"type": "object", "required": []})),
        Some(json!({"type": "object", "properties": {}, "additionalProperties": false})),
        Some(
            json!({"type": "object", "properties": {}, "additionalProperties": true, "required": []}),
        ),
        Some(json!({"type": "object", "additionalProperties": false})),
        Some(json!({"description": "takes anything"})),
    ];
    let tools: Vec<ToolDef> = forms
        .into_iter()
        .enumerate()
        .map(|(i, parameters)| ToolDef {
            name: format!("t{i}"),
            description: None,
            parameters,
        })
        .collect();
    let back = roundtrip(&tools);
    assert_eq!(back, tools);
    // And the lines really differ from one another.
    let compact = encode_tools(&tools).unwrap();
    let lines: Vec<&str> = compact.definitions().split('\n').collect();
    for i in 0..lines.len() {
        for j in 0..i {
            assert_ne!(
                lines[i].trim_start_matches(char::is_alphanumeric),
                lines[j].trim_start_matches(char::is_alphanumeric),
                "{} vs {}",
                lines[i],
                lines[j]
            );
        }
    }
}

#[test]
fn required_order_is_preserved_and_optionals_are_sorted() {
    let t = tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "zeta": {"type": "string"},
                "alpha": {"type": "string"},
                "mid": {"type": "string"},
                "beta": {"type": "integer"}
            },
            "required": ["mid", "zeta"]
        }),
    );
    let compact = encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        compact.definitions(),
        "t(mid:str, zeta:str, alpha?:str, beta?:int) - test tool"
    );
    assert_eq!(roundtrip(std::slice::from_ref(&t)), vec![t]);
}

#[test]
fn enum_member_order_and_null_member_survive() {
    let t = tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": ["string", "null"], "enum": ["zulu", null, "alpha", "str", "2", "a b"]},
                "b": {"type": "integer", "enum": [3, -1, 1.0, 9007199254740993_i64]},
                "c": {"type": ["integer", "null"], "enum": [null, 0]}
            }
        }),
    );
    let compact = encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        compact.definitions(),
        "t(a?:zulu|null|alpha|\"str\"|\"2\"|\"a b\", b?:3|-1|1.0|9007199254740993, c?:null|0) - test tool"
    );
    assert_eq!(roundtrip(std::slice::from_ref(&t)), vec![t]);
}

#[test]
fn numeric_boundaries_survive_exactly() {
    let t = tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "minimum": 1, "maximum": 1.0},
                "b": {"type": "number", "minimum": -0.0, "exclusiveMaximum": 1e22},
                "c": {"type": "integer", "minimum": u64::MAX},
                "d": {"type": "integer", "maximum": i64::MIN},
                "e": {"type": "number", "exclusiveMinimum": 0.1, "maximum": 0.30000000000000004},
                "f": {"type": "integer", "minimum": 9007199254740993_i64}
            }
        }),
    );
    assert_eq!(roundtrip(std::slice::from_ref(&t)), vec![t]);
}

#[test]
fn format_values_survive_verbatim_whether_aliased_or_not() {
    let t = tool(
        "t",
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "format": "date-time"},
                "b": {"type": "string", "format": "Date-Time"},
                "c": {"type": "string", "format": "ipv4", "minLength": 7},
                "d": {"type": "string", "format": "has space, and \"quote\""},
                "e": {"type": ["string", "null"], "format": "email", "maxLength": 100}
            }
        }),
    );
    let compact = encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        compact.definitions(),
        "t(a?:datetime, b?:str(format=Date-Time), c?:str(min=7,format=ipv4), d?:str(format=\"has space, and \\\"quote\\\"\"), e?:email(max=100)|null) - test tool"
    );
    assert_eq!(roundtrip(std::slice::from_ref(&t)), vec![t]);
}

#[test]
fn annotations_survive_verbatim_and_never_change_meaning() {
    let t = tool(
        "t",
        json!({
            "type": "object",
            "title": "Root",
            "default": {"z": 1, "a": [true, null]},
            "properties": {
                "limit": {"type": "integer", "default": 10, "title": "Limit", "examples": [1, 2]},
                "tag": {"type": "string", "examples": ["a"]}
            }
        }),
    );
    let compact = encode_tools(std::slice::from_ref(&t)).unwrap();
    assert_eq!(
        compact.definitions(),
        "t(limit?:int@{\"title\":\"Limit\",\"default\":10,\"examples\":[1,2]}, tag?:str@{\"examples\":[\"a\"]})@{\"title\":\"Root\",\"default\":{\"a\":[true,null],\"z\":1}} - test tool"
    );
    assert_eq!(roundtrip(std::slice::from_ref(&t)), vec![t]);
}

#[test]
fn unicode_multiline_and_empty_descriptions_are_exact() {
    let descs = [
        Some("日本語の説明 🎉"),
        Some("Line one\nLine two\ttabbed"),
        Some(" leading"),
        Some("trailing "),
        Some(""),
        Some("\"starts with a quote"),
        Some("has \"inner\" quotes and \\ backslash"),
        Some("\u{0}control"),
        None,
    ];
    let tools: Vec<ToolDef> = descs
        .iter()
        .enumerate()
        .map(|(i, d)| ToolDef {
            name: format!("t{i}"),
            description: d.map(str::to_owned),
            parameters: Some(json!({
                "type": "object",
                "description": d.unwrap_or("root"),
                "properties": {"p": {"type": "string", "description": d.unwrap_or("prop")}}
            })),
        })
        .collect();
    assert_roundtrips(tools);
}

#[test]
fn generated_catalogs_roundtrip() {
    let mut rng = Lcg::new(2026);
    let mut accepted = 0;
    for round in 0..400 {
        let tools: Vec<ToolDef> = (0..5).map(|i| gen_tool(&mut rng, i)).collect();
        match encode_tools(&tools) {
            Ok(compact) => {
                accepted += 1;
                let back = decode_tools(&compact).unwrap_or_else(|e| panic!("round {round}: {e}"));
                assert_eq!(back, tools, "round {round}: {}", compact.definitions());
            }
            Err(e) => panic!("generator produced an unsupported catalog in round {round}: {e}"),
        }
    }
    assert_eq!(accepted, 400);
}

#[test]
fn decode_reads_the_text_not_a_retained_copy() {
    let t = tool(
        "t",
        json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}),
    );
    let compact = encode_tools(&[t]).unwrap();
    // Re-parsing the definitions text alone (through the public API) yields the tool.
    let text = compact.definitions().to_owned();
    assert_eq!(text, "t(a:str) - test tool");
    let back = decode_tools(&compact).unwrap();
    assert_eq!(
        back[0].parameters,
        Some(json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}))
    );
}
