//! Catalog-level rules: names, duplicates, counts and sizes, surfaced at `encode_tools` and
//! `StreamDecoder::new` alike.

mod common;

use common::tool;
use nasiko_tool_compact::{StreamDecoder, ToolCompactError, ToolDef, encode_tools, limits};
use serde_json::json;

#[test]
fn duplicate_names_are_an_invalid_catalog() {
    let tools = vec![
        tool("same", json!({"type": "object"})),
        tool("same", json!({})),
    ];
    let err = encode_tools(&tools).unwrap_err();
    assert_eq!(err.kind(), "invalid_catalog");
    assert!(StreamDecoder::new(&tools).is_err());
}

#[test]
fn bad_names_are_an_invalid_catalog() {
    for name in [
        "",
        "has space",
        "ünï",
        "a/b",
        &"x".repeat(limits::MAX_NAME_LEN + 1),
    ] {
        let err = encode_tools(&[tool(name, json!({"type": "object"}))]).unwrap_err();
        assert_eq!(err.kind(), "invalid_catalog", "name {name:?}");
    }
    assert!(
        encode_tools(&[tool(
            &"x".repeat(limits::MAX_NAME_LEN),
            json!({"type": "object"})
        )])
        .is_ok()
    );
    assert!(encode_tools(&[tool("9starts.with-digit", json!({"type": "object"}))]).is_ok());
}

#[test]
fn the_decoder_surfaces_catalog_errors_before_any_text() {
    let unsupported = tool(
        "t",
        json!({"type": "object", "properties": {"a": {"type": "string", "pattern": "x"}}}),
    );
    match StreamDecoder::new(&[unsupported]) {
        Err(e) => assert_eq!(e.kind(), "unsupported_schema"),
        Ok(_) => panic!("decoder accepted an unsupported catalog"),
    }
}

#[test]
fn the_empty_catalog_is_valid_and_decodes_only_prose() {
    let compact = encode_tools(&[]).unwrap();
    assert_eq!(compact.definitions(), "");
    assert!(compact.tools().is_empty());
    let mut d = StreamDecoder::new(&[]).unwrap();
    d.push("just prose").unwrap();
    assert_eq!(d.finish().unwrap().content, "just prose");
}

#[test]
fn tool_count_limit_is_exact() {
    let tools: Vec<ToolDef> = (0..limits::MAX_TOOLS)
        .map(|i| tool(&format!("t{i}"), json!({"type": "object"})))
        .collect();
    assert!(encode_tools(&tools).is_ok());
    let mut more = tools;
    more.push(tool("extra", json!({"type": "object"})));
    assert!(matches!(
        encode_tools(&more),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_TOOLS",
            ..
        })
    ));
}

#[test]
fn serialized_schema_size_limit_is_checked_before_lowering() {
    // Each description is under MAX_DESCRIPTION_BYTES, so only the aggregate size can trip.
    let desc = "d".repeat(limits::MAX_DESCRIPTION_BYTES - 100);
    let tools: Vec<ToolDef> = (0..limits::MAX_TOOLS)
        .map(|i| ToolDef {
            name: format!("t{i}"),
            description: Some(desc.clone()),
            parameters: Some(json!({"type": "object"})),
        })
        .collect();
    assert!(matches!(
        encode_tools(&tools),
        Err(ToolCompactError::LimitExceeded {
            limit: "MAX_SCHEMA_BYTES",
            ..
        })
    ));
}
