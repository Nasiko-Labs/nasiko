# Task 4 — Encode one tool

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`encode.rs`: `encode_tools` classifies, renders each signature, joins them with newlines,
and appends one instruction line. Rendering rules:

- property order follows the `properties` object order
- required field is `name:type`, optional is `name?:type`
- `string` → `str`, `integer` → `int`, `number` → `num`, `boolean` → `bool`,
  `string` + `date-time` → `datetime`
- string enum → `public|private` with no extra spaces
- array → `[item]`
- nested object → `{inner, ...}` using the same required mark
- description is the tool description, or an empty string when absent, still preceded by ` - `

`CompactTools.text` is that block. Store the original `ToolDef`s on the struct.

### Tests

```rust
#[test]
fn calendar_signature_marks_required_optional_and_enum() {
    let compact = encode_tools(&[calendar()]).unwrap();
    let line = compact.text.lines().next().unwrap();
    assert!(line.starts_with("create_calendar_event("));
    assert!(line.contains("title:str"));
    assert!(line.contains("start:datetime"));
    assert!(line.contains("duration_min?:int"));
    assert!(line.contains("attendees?:[str]"));
    assert!(line.contains("visibility?:public|private"));
    assert!(line.ends_with(" - Create an event in the user's calendar."));
    assert!(compact.text.contains("To call a tool, emit: <<call name {json args}>>"));
}

#[test]
fn encoding_a_ref_tool_returns_unsupported_schema() {
    let tool = ToolDef {
        name: "lookup".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {"id": {"$ref": "#/$defs/Id"}}})),
    };
    assert!(matches!(encode_tools(&[tool]), Err(CompactError::UnsupportedSchema { .. })));
}
```
