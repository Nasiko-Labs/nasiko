# Task 5 — Schema round trip

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`decode_tools` parses `CompactTools.text` signatures back into `Shape`s and rebuilds a
JSON Schema `ToolDef` per tool. It does not read the stored originals. Compare facts,
not key order: name, description, required set, and each property's type, format, enum,
and array item type.

If the stored originals disagree with the parsed text, that is a bug in the renderer.
The test must fail in that case, so `decode_tools` must not return the stored vec.

### Tests

```rust
#[test]
fn decoded_calendar_keeps_schema_facts() {
    let compact = encode_tools(&[calendar()]).unwrap();
    let decoded = decode_tools(&compact).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].name, "create_calendar_event");
    assert_eq!(decoded[0].description, calendar().description);
    let params = decoded[0].parameters.as_ref().unwrap();
    let required = params["required"].as_array().unwrap();
    assert!(required.contains(&json!("title")) && required.contains(&json!("start")));
    assert_eq!(required.len(), 2);
    assert_eq!(params["properties"]["start"]["format"], "date-time");
    assert_eq!(params["properties"]["visibility"]["enum"], json!(["public", "private"]));
    assert_eq!(params["properties"]["attendees"]["items"]["type"], "string");
    assert_eq!(params["properties"]["duration_min"]["type"], "integer");
}

#[test]
fn two_tools_keep_both_names() {
    let email = ToolDef {
        name: "send_email".into(),
        description: Some("Send an email.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": { "to": {"type": "array", "items": {"type": "string"}} },
            "required": ["to"]
        })),
    };
    let compact = encode_tools(&[calendar(), email]).unwrap();
    let names: Vec<_> = decode_tools(&compact).unwrap().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["create_calendar_event", "send_email"]);
}
```
