//! Step 9 stress tests: gateway aliasing, round-trip, collisions, args.
//!
//! Real MCP catalogs are loaded from `/tmp/p1_mcp_catalogs/*_openai_tools.json`
//! when present. Synthetic fixtures always run.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use nasiko_tool_compact::{
    CompactError, ToolCall, ToolDef, assign_aliases, decode_calls, encode_tools,
    is_valid_compact_ident, parse_gateway_tool_name, render_calls,
};
use serde_json::{Value, json};

const CATALOG_DIR: &str = "/tmp/p1_mcp_catalogs";
const CATALOGS: &[&str] = &[
    "calendar_openai_tools.json",
    "email_openai_tools.json",
    "github_openai_tools.json",
    "general_openai_tools.json",
];

fn load_openai_tools(path: &Path) -> Vec<ToolDef> {
    let v: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let f = &t["function"];
            ToolDef {
                name: f["name"].as_str().unwrap().into(),
                description: f
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(str::to_string),
                parameters: f.get("parameters").cloned(),
            }
        })
        .collect()
}

fn catalogs_available() -> bool {
    CATALOGS
        .iter()
        .all(|f| Path::new(CATALOG_DIR).join(f).is_file())
}

fn minimal_valid_args(parameters: Option<&Value>) -> Value {
    let Some(schema) = parameters else {
        return json!({});
    };
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut out = serde_json::Map::new();
    for key in required {
        let sample = props
            .get(&key)
            .map(sample_for_schema)
            .unwrap_or_else(|| json!("x"));
        out.insert(key, sample);
    }
    Value::Object(out)
}

fn sample_for_schema(schema: &Value) -> Value {
    if let Some(arr) = schema.get("enum").and_then(|e| e.as_array()) {
        if let Some(first) = arr.first() {
            return first.clone();
        }
    }
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => json!("sample"),
        Some("integer") | Some("number") => json!(1),
        Some("boolean") => json!(true),
        Some("array") => {
            let item = schema
                .get("items")
                .map(sample_for_schema)
                .unwrap_or(json!("x"));
            json!([item])
        }
        Some("object") => {
            let mut m = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                let required = schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<HashSet<_>>())
                    .unwrap_or_default();
                for (k, v) in props {
                    if required.contains(k.as_str()) {
                        m.insert(k.clone(), sample_for_schema(v));
                    }
                }
            }
            Value::Object(m)
        }
        _ => json!("sample"),
    }
}

// ── Alias correctness ────────────────────────────────────────────────────────

#[test]
fn alias_normal_uuid_prefixed() {
    let names = vec!["1b9dbb038d23478e__calendar_create_event".into()];
    let aliases = assign_aliases(&names).unwrap();
    assert_eq!(aliases[0].0, "calendar_create_event");
    assert_eq!(aliases[0].1, names[0]);
}

#[test]
fn alias_valid_suffix_unique() {
    let names = vec![
        "aaaaaaaaaaaaaaaa__alpha".into(),
        "aaaaaaaaaaaaaaaa__beta".into(),
    ];
    let aliases = assign_aliases(&names).unwrap();
    assert_eq!(aliases[0].0, "alpha");
    assert_eq!(aliases[1].0, "beta");
}

#[test]
fn alias_same_suffix_collision_never_collapses() {
    let names = vec![
        "aaaaaaaaaaaaaaaa__create".into(),
        "bbbbbbbbbbbbbbbb__create".into(),
    ];
    let a1 = assign_aliases(&names).unwrap();
    let a2 = assign_aliases(&names).unwrap();
    assert_eq!(a1, a2, "collision aliases must be deterministic");
    assert_ne!(a1[0].0, a1[1].0);
    assert!(!a1.iter().any(|(c, _)| c == "create"));
    assert_eq!(a1[0].0, "create_aaaaaaaaaaaaaaaa");
    assert_eq!(a1[1].0, "create_bbbbbbbbbbbbbbbb");
    // reversible
    let map: HashMap<_, _> = a1.iter().cloned().collect();
    assert_eq!(
        map.get("create_aaaaaaaaaaaaaaaa").map(String::as_str),
        Some("aaaaaaaaaaaaaaaa__create")
    );
    assert_eq!(
        map.get("create_bbbbbbbbbbbbbbbb").map(String::as_str),
        Some("bbbbbbbbbbbbbbbb__create")
    );
}

#[test]
fn alias_malformed_short_hex_fails_closed() {
    assert!(parse_gateway_tool_name("12345678__create").is_none());
    assert!(matches!(
        assign_aliases(&["12345678__create".into()]),
        Err(CompactError::InvalidToolName(_))
    ));
}

#[test]
fn alias_malformed_non_hex_prefix_fails_closed() {
    assert!(parse_gateway_tool_name("zzzzzzzzzzzzzzzz__create").is_none());
    assert!(matches!(
        assign_aliases(&["zzzzzzzzzzzzzzzz__create".into()]),
        Err(CompactError::InvalidToolName(_))
    ));
}

#[test]
fn alias_empty_suffix_fails_closed() {
    assert!(parse_gateway_tool_name("aaaaaaaaaaaaaaaa__").is_none());
    assert!(matches!(
        assign_aliases(&["aaaaaaaaaaaaaaaa__".into()]),
        Err(CompactError::InvalidToolName(_))
    ));
}

#[test]
fn alias_digit_leading_suffix_fails_closed() {
    assert!(matches!(
        assign_aliases(&["aaaaaaaaaaaaaaaa__9create".into()]),
        Err(CompactError::InvalidToolName(_))
    ));
}

#[test]
fn alias_digit_leading_compact_ident_rejected() {
    assert!(!is_valid_compact_ident("9bad"));
    assert!(!is_valid_compact_ident("1create"));
}

#[test]
fn alias_invalid_characters_fail_closed() {
    for name in [
        "aaaaaaaaaaaaaaaa__bad-name",
        "aaaaaaaaaaaaaaaa__bad.name",
        "aaaaaaaaaaaaaaaa__bad name",
        "plain-bad",
        "plain.bad",
    ] {
        assert!(
            matches!(
                assign_aliases(&[name.into()]),
                Err(CompactError::InvalidToolName(_))
            ),
            "expected fail closed for {name}"
        );
    }
}

#[test]
fn alias_duplicate_plain_names_fail_closed() {
    let names = vec!["create".into(), "create".into()];
    assert!(matches!(
        assign_aliases(&names),
        Err(CompactError::InvalidToolName(_))
    ));
}

#[test]
fn unknown_alias_resolve_none() {
    let tools = vec![ToolDef {
        name: "aaaaaaaaaaaaaaaa__ping".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {}})),
    }];
    let c = encode_tools(&tools).unwrap();
    assert!(c.resolve_original_name("nope").is_none());
    assert_eq!(
        decode_calls("<<nope {}>>", c.tools()).unwrap_err(),
        CompactError::UnknownTool
    );
}

#[test]
fn missing_reverse_mapping_is_detectable() {
    // CompactTools always stores identity/gateway maps for encoded tools.
    // A missing map entry must never be guessed.
    let tools = vec![ToolDef {
        name: "ping".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {}})),
    }];
    let c = encode_tools(&tools).unwrap();
    assert_eq!(c.resolve_original_name("ping"), Some("ping"));
    assert!(c.resolve_original_name("ghost").is_none());
}

// ── Round-trip across real catalogs ──────────────────────────────────────────

#[test]
fn real_catalog_roundtrip_every_tool() {
    if !catalogs_available() {
        eprintln!("skip: real MCP catalogs not present under {CATALOG_DIR}");
        return;
    }
    for file in CATALOGS {
        let path = Path::new(CATALOG_DIR).join(file);
        let tools = load_openai_tools(&path);
        let encoded = encode_tools(&tools).expect("catalog must compact");
        assert_eq!(encoded.tools().len(), tools.len());

        for (original, aliased) in tools.iter().zip(encoded.tools().iter()) {
            assert!(
                !aliased
                    .name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit()),
                "compact alias must not be digit-leading: {}",
                aliased.name
            );
            let args = minimal_valid_args(aliased.parameters.as_ref());
            let marker = format!(
                "<<{} {}>>",
                aliased.name,
                serde_json::to_string(&args).unwrap()
            );
            let calls = decode_calls(&marker, encoded.tools()).unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].name, aliased.name);
            let resolved = encoded
                .resolve_original_name(&calls[0].name)
                .expect("reverse map must exist");
            assert_eq!(
                resolved, original.name,
                "round-trip failed for {} in {file}",
                original.name
            );
        }
    }
}

// ── Multiple calls ───────────────────────────────────────────────────────────

#[test]
fn multiple_calls_preserve_order_and_resolve() {
    let tools = vec![
        ToolDef {
            name: "1b9dbb038d23478e__calendar_create_event".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string"},
                    "start_time": {"type": "string"}
                },
                "required": ["title", "start_time"]
            })),
        },
        ToolDef {
            name: "1b9dbb038d23478e__calendar_list_events".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": { "date": {"type": "string"} },
                "required": ["date"]
            })),
        },
        ToolDef {
            name: "finish_task".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": { "message": {"type": "string"} },
                "required": ["message"]
            })),
        },
    ];
    let encoded = encode_tools(&tools).unwrap();
    let text = concat!(
        r#"<<calendar_create_event {"title":"Team Meeting","start_time":"2026-10-04T15:00:00"}>>"#,
        "\n",
        r#"<<calendar_list_events {"date":"2026-10-04"}>>"#,
        "\n",
        r#"<<finish_task {"message":"done"}>>"#,
    );
    let calls = decode_calls(text, encoded.tools()).unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].name, "calendar_create_event");
    assert_eq!(calls[1].name, "calendar_list_events");
    assert_eq!(calls[2].name, "finish_task");

    let resolved: Vec<&str> = calls
        .iter()
        .map(|c| encoded.resolve_original_name(&c.name).unwrap())
        .collect();
    assert_eq!(
        resolved,
        [
            "1b9dbb038d23478e__calendar_create_event",
            "1b9dbb038d23478e__calendar_list_events",
            "finish_task",
        ]
    );
}

// ── Argument validation on real tools ────────────────────────────────────────

#[test]
fn real_catalog_argument_validation_fail_closed() {
    if !catalogs_available() {
        eprintln!("skip: real MCP catalogs not present under {CATALOG_DIR}");
        return;
    }
    for file in CATALOGS {
        let tools = load_openai_tools(&Path::new(CATALOG_DIR).join(file));
        let encoded = encode_tools(&tools).unwrap();
        for tool in encoded.tools() {
            let valid = minimal_valid_args(tool.parameters.as_ref());
            let ok = format!(
                "<<{} {}>>",
                tool.name,
                serde_json::to_string(&valid).unwrap()
            );
            assert!(
                decode_calls(&ok, encoded.tools()).is_ok(),
                "valid args rejected for {} in {file}",
                tool.name
            );

            // missing required
            if let Some(req) = tool
                .parameters
                .as_ref()
                .and_then(|p| p.get("required"))
                .and_then(|r| r.as_array())
            {
                if let Some(first) = req.first().and_then(|x| x.as_str()) {
                    let mut bad = valid.clone();
                    bad.as_object_mut().unwrap().remove(first);
                    let marker =
                        format!("<<{} {}>>", tool.name, serde_json::to_string(&bad).unwrap());
                    assert_eq!(
                        decode_calls(&marker, encoded.tools()).unwrap_err(),
                        CompactError::InvalidArguments,
                        "missing required must fail for {}",
                        tool.name
                    );
                }
            }

            // wrong primitive type on first required string field
            if let Some(props) = tool
                .parameters
                .as_ref()
                .and_then(|p| p.get("properties"))
                .and_then(|p| p.as_object())
            {
                if let Some((key, schema)) = props.iter().find(|(_, s)| {
                    s.get("type").and_then(|t| t.as_str()) == Some("string")
                        && s.get("enum").is_none()
                }) {
                    let mut bad = valid.clone();
                    bad.as_object_mut().unwrap().insert(key.clone(), json!(123));
                    let marker =
                        format!("<<{} {}>>", tool.name, serde_json::to_string(&bad).unwrap());
                    assert_eq!(
                        decode_calls(&marker, encoded.tools()).unwrap_err(),
                        CompactError::InvalidArguments,
                        "wrong type must fail for {}.{}",
                        tool.name,
                        key
                    );
                    let _ = schema;
                }
                // invalid enum if present
                if let Some((key, schema)) = props
                    .iter()
                    .find(|(_, s)| s.get("enum").and_then(|e| e.as_array()).is_some())
                {
                    let mut bad = valid.clone();
                    bad.as_object_mut()
                        .unwrap()
                        .insert(key.clone(), json!("__not_an_enum_value__"));
                    let marker =
                        format!("<<{} {}>>", tool.name, serde_json::to_string(&bad).unwrap());
                    assert_eq!(
                        decode_calls(&marker, encoded.tools()).unwrap_err(),
                        CompactError::InvalidArguments
                    );
                    let _ = schema;
                }
            }

            // malformed JSON
            let malformed = format!("<<{} {{title:>>", tool.name);
            assert!(decode_calls(&malformed, encoded.tools()).is_err());

            // non-object arguments — fail closed (no ToolCall invented)
            let non_obj = format!("<<{} []>>", tool.name);
            assert!(
                matches!(
                    decode_calls(&non_obj, encoded.tools()),
                    Err(CompactError::InvalidArguments | CompactError::MalformedCall)
                ),
                "non-object args must fail closed for {}",
                tool.name
            );
        }
    }
}

#[test]
fn surface_formats_never_become_tool_calls() {
    let tools = vec![ToolDef {
        name: "aaaaaaaaaaaaaaaa__calendar_create_event".into(),
        description: None,
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "start_time": {"type": "string"}
            },
            "required": ["title", "start_time"]
        })),
    }];
    let encoded = encode_tools(&tools).unwrap();
    let surfaces = [
        r#"{"tool_slug":"calendar_create_event","args":{"title":"t","start_time":"s"}}"#,
        "```json\n{\"name\":\"calendar_create_event\"}\n```",
        r#"CALL calendar_create_event {"title":"t","start_time":"s"}"#,
        "Sure, I can create that meeting for you.",
        "<<calendar_create_event {title:t}>>",
        "<<unknown_alias {\"title\":\"t\",\"start_time\":\"s\"}>>",
    ];
    for text in surfaces {
        match decode_calls(text, encoded.tools()) {
            Ok(calls) => assert!(
                calls.is_empty(),
                "must not invent ToolCalls for surface={text:?} got={calls:?}"
            ),
            Err(CompactError::UnknownTool | CompactError::InvalidArguments) => {}
            Err(e) => panic!("unexpected error for {text:?}: {e}"),
        }
    }
}

#[test]
fn collision_encode_roundtrip_reversible() {
    let tools = vec![
        ToolDef {
            name: "aaaaaaaaaaaaaaaa__create".into(),
            description: None,
            parameters: Some(json!({"type":"object","properties":{}})),
        },
        ToolDef {
            name: "bbbbbbbbbbbbbbbb__create".into(),
            description: None,
            parameters: Some(json!({"type":"object","properties":{}})),
        },
    ];
    let encoded = encode_tools(&tools).unwrap();
    assert_eq!(encoded.tools()[0].name, "create_aaaaaaaaaaaaaaaa");
    assert_eq!(encoded.tools()[1].name, "create_bbbbbbbbbbbbbbbb");
    let calls = vec![
        ToolCall {
            name: "create_aaaaaaaaaaaaaaaa".into(),
            arguments: json!({}),
        },
        ToolCall {
            name: "create_bbbbbbbbbbbbbbbb".into(),
            arguments: json!({}),
        },
    ];
    let text = render_calls(&calls).unwrap();
    let decoded = decode_calls(&text, encoded.tools()).unwrap();
    assert_eq!(decoded.len(), 2);
    assert_eq!(
        encoded.resolve_original_name(&decoded[0].name),
        Some("aaaaaaaaaaaaaaaa__create")
    );
    assert_eq!(
        encoded.resolve_original_name(&decoded[1].name),
        Some("bbbbbbbbbbbbbbbb__create")
    );
}
